//! Bounded text evidence for focused questions when a callable cannot be verified.

use super::*;

const ENTRY_LIMIT: usize = 4_096;
const FILE_LIMIT: usize = 384;
const FILE_BYTES: usize = 192 * 1024;
const SCAN_BYTES: usize = 12 * 1024 * 1024;
const TERM_LIMIT: usize = 20;
const WINDOWS_PER_FILE: usize = 4;
const RADIUS: usize = 2;

#[derive(Clone, Copy)]
struct Window {
    line: usize,
    exact: u32,
    nearby: u32,
}

struct CandidateFile {
    path: PathBuf,
    hash: String,
    path_terms: u32,
    windows: Vec<Window>,
}

pub(super) fn live_candidates(
    root: &Path,
    question: &str,
    file_globs: &[String],
    limit: usize,
    body_lines: Option<usize>,
    max_bytes: usize,
) -> Result<String, ApiError> {
    let terms = question_terms(question);
    let mut out = String::from("Live text candidates. Call relationships unavailable here.\n");
    if terms.is_empty() {
        out.push_str("No searchable terms in the question. Try an exact name or path.\n");
        return Ok(bound_reply(out, max_bytes, &[]));
    }
    // Count visited directories as well as files so a sparse glob also has
    // a work limit. Match the ordinary live walk's ignore configuration.
    let mut walk = ignore::WalkBuilder::new(root);
    walk.add_custom_ignore_filename(".rgignore");
    if let Some(scope) = lookup_scope(root, file_globs)? {
        walk.overrides(scope);
    }
    let canonical_root = root.canonicalize().map_err(|err| {
        ApiError::invalid_argument(format!("Could not resolve the project: {err}"))
    })?;
    let mut files = Vec::new();
    let mut storage = Vec::new();
    let mut frequency = [0usize; TERM_LIMIT];
    let mut examined = 0;
    let mut read = 0usize;
    let mut bytes = 0;
    let mut skipped = 0;
    let mut limited = false;
    for (number, entry) in walk.build().enumerate() {
        if number == ENTRY_LIMIT || examined == FILE_LIMIT || bytes == SCAN_BYTES {
            limited = true;
            break;
        }
        let Ok(entry) = entry else {
            skipped += 1;
            continue;
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        examined += 1;
        let path = entry.into_path();
        if !path
            .to_str()
            .is_some_and(|path| !path.chars().any(char::is_control))
        {
            skipped += 1;
            continue;
        }
        let loaded = load_live(
            &canonical_root,
            &path,
            (SCAN_BYTES - bytes).min(FILE_BYTES),
            &mut storage,
        );
        let Some(text) = loaded else {
            bytes += storage.len();
            skipped += 1;
            continue;
        };
        bytes += text.len();
        read += 1;
        let masks = text
            .lines()
            .map(|line| tokens(line, &terms))
            .collect::<Vec<_>>();
        let present = masks.iter().fold(0, |all, mask| all | mask);
        for (offset, count) in frequency.iter_mut().enumerate() {
            *count += usize::from(present & (1 << offset) != 0);
        }
        let windows = ranked_windows(&masks);
        if !windows.is_empty() {
            files.push(CandidateFile {
                hash: sha256_hex(text.as_bytes()),
                path_terms: tokens(&project_path(root, &path), &terms),
                path,
                windows,
            });
        }
    }
    let weights = frequency.map(|count| (read + 1).div_ceil(count + 1).min(8));
    let mut ranked = files
        .iter()
        .flat_map(|file| file.windows.iter().map(move |window| (file, window)))
        .collect::<Vec<_>>();
    ranked.sort_unstable_by_key(|&(file, window)| {
        let score = |mask: u32| {
            weights
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, w)| w)
                .sum::<usize>()
        };
        let priority =
            score(window.nearby) + score(window.exact) + score(file.path_terms & window.nearby);
        (
            Reverse(priority),
            Reverse(window.nearby.count_ones()),
            &file.path,
            window.line,
        )
    });

    let mut excerpts = String::new();
    let mut follow_up = None;
    let mut source_lines = body_lines.unwrap_or(0);
    let mut shown = 0;
    let mut drifted = 0;
    // Revalidate only a bounded shortlist before displaying any excerpt.
    for (file, window) in ranked.iter().take(limit.saturating_mul(2).min(40)) {
        if shown == limit {
            break;
        }
        let Some(text) = load_live(&canonical_root, &file.path, FILE_BYTES, &mut storage)
            .filter(|text| sha256_hex(text.as_bytes()) == file.hash)
        else {
            drifted += 1;
            continue;
        };
        let lines = text.lines().collect::<Vec<_>>();
        let first = window.line.saturating_sub(RADIUS);
        let end = (window.line + RADIUS + 1).min(lines.len());
        let path = printable(&project_path(root, &file.path));
        shown += 1;
        let _ = writeln!(out, "{shown}. {path}:{}-{end}", first + 1);
        follow_up.get_or_insert_with(|| format!("follow-up: read {path}:{}-{end}\n", first + 1));
        let count = (end - first).min(source_lines);
        if count > 0 {
            let start = first
                .max(window.line.saturating_sub(count / 2))
                .min(end - count);
            let _ = writeln!(excerpts, "== {path}:{}", start + 1);
            for (offset, line) in lines[start..start + count].iter().enumerate() {
                let _ = writeln!(excerpts, "{}: {}", start + offset + 1, bounded(line));
            }
            source_lines -= count;
        }
    }
    if shown == 0 {
        out.push_str(
            "No verified live text matches in the scanned files. Try an exact name or path.\n",
        );
    }
    let _ = writeln!(
        out,
        "Coverage: {read} text files read, {skipped} skipped, {drifted} changed before display, {} candidate windows not shown. Scan limit reached: {limited}.",
        ranked.len() - shown,
    );
    out.push_str(&excerpts);
    Ok(bound_reply(out, max_bytes, &[follow_up.as_deref()]))
}

pub(super) fn load_live<'a>(
    root: &Path,
    path: &Path,
    limit: usize,
    bytes: &'a mut Vec<u8>,
) -> Option<&'a str> {
    bytes.clear();
    let canonical = path.canonicalize().ok()?;
    if !canonical.starts_with(root) || !std::fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    let file = std::fs::File::open(&canonical).ok()?;
    let before = file.metadata().ok()?;
    if !before.is_file() || before.len() > limit as u64 {
        return None;
    }
    (&file).take(limit as u64).read_to_end(bytes).ok()?;
    let after = file.metadata().ok()?;
    if bytes.len() as u64 != before.len()
        || before.len() != after.len()
        || before.modified().ok()? != after.modified().ok()?
        || path.canonicalize().ok()? != canonical
        || bytes.contains(&0)
    {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

fn ranked_windows(masks: &[u32]) -> Vec<Window> {
    let mut best: Vec<Window> = Vec::new();
    let key = |window: &Window| {
        (
            window.nearby.count_ones(),
            window.exact.count_ones(),
            Reverse(window.line),
        )
    };
    for (line, &exact) in masks.iter().enumerate().filter(|(_, exact)| **exact != 0) {
        let span = line.saturating_sub(RADIUS)..(line + RADIUS + 1).min(masks.len());
        let nearby = masks[span].iter().fold(0, |all, mask| all | mask);
        let next = Window {
            line,
            exact,
            nearby,
        };
        if best
            .iter()
            .any(|old| old.line.abs_diff(line) <= RADIUS * 2 && key(old) >= key(&next))
        {
            continue;
        }
        best.retain(|old| old.line.abs_diff(line) > RADIUS * 2);
        best.push(next);
        best.sort_unstable_by_key(|window| Reverse(key(window)));
        best.truncate(WINDOWS_PER_FILE);
    }
    best
}

fn question_terms(question: &str) -> HashMap<String, u32> {
    let mut terms = HashMap::new();
    visit_words(question, |word| {
        let term = normalized(word);
        if terms.len() < TERM_LIMIT
            && term.chars().count() >= 2
            && !matches!(
                term.as_str(),
                "a" | "an"
                    | "and"
                    | "are"
                    | "can"
                    | "do"
                    | "does"
                    | "for"
                    | "how"
                    | "in"
                    | "is"
                    | "it"
                    | "of"
                    | "on"
                    | "or"
                    | "set"
                    | "that"
                    | "the"
                    | "this"
                    | "to"
                    | "use"
                    | "what"
                    | "when"
                    | "where"
                    | "which"
                    | "with"
            )
        {
            let bit = 1 << terms.len();
            terms.entry(term).or_insert(bit);
        }
    });
    terms
}

fn normalized(word: &str) -> String {
    let mut lower = word.to_lowercase();
    if lower.len() >= 5 && lower.ends_with("ies") {
        lower.truncate(lower.len() - 3);
        lower.push('y');
    } else if lower.len() >= 5 && lower.ends_with('s') && !lower.ends_with("ss") {
        lower.pop();
    }
    lower
}

fn tokens(text: &str, terms: &HashMap<String, u32>) -> u32 {
    let mut matches = 0;
    visit_words(text, |word| {
        matches |= terms.get(&normalized(word)).copied().unwrap_or(0)
    });
    matches
}

fn visit_words(text: &str, mut visit: impl FnMut(&str)) {
    for word in text
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let mut chars = word.char_indices().peekable();
        let mut last: Option<char> = None;
        let mut start = 0;
        while let Some((offset, current)) = chars.next() {
            if current.is_uppercase()
                && last.is_some_and(|previous| {
                    previous.is_lowercase()
                        || (previous.is_uppercase()
                            && chars.peek().is_some_and(|(_, next)| next.is_lowercase()))
                })
            {
                visit(&word[start..offset]);
                start = offset;
            }
            last = Some(current);
        }
        visit(&word[start..]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rare_nearby_terms_locate_unindexed_config_with_scope_and_byte_limits() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        std::fs::create_dir(root.join("settings")).unwrap();
        std::fs::write(root.join(".rgignore"), "ignored.env\n").unwrap();
        std::fs::write(root.join("ignored.env"), "CAFÉ_RETRY_DELAYS=true\n").unwrap();
        std::fs::write(
            root.join("settings/default.yml"),
            "retry_policy: standard\n",
        )
        .unwrap();
        std::fs::write(
            root.join("settings/region.toml"),
            "region = \"north\"\nCaféRetryDelay = 30\nlabel = \"Việt Nam\"\n",
        )
        .unwrap();
        let question = "Where are café retry delays set?";

        let reply = live_candidates(root, question, &[], 1, Some(10), 7_500).unwrap();
        assert!(reply.contains("1. settings/region.toml:1-3"), "{reply}");
        assert!(reply.contains("2: CaféRetryDelay = 30"), "{reply}");
        assert!(reply.contains("3: label = \"Việt Nam\""), "{reply}");
        assert!(
            !reply.contains("ignored.env") && !reply.contains("default.yml"),
            "{reply}"
        );

        let scope = vec!["**/*.yml".into()];
        let compact = live_candidates(root, question, &scope, 1, None, 7_500).unwrap();
        assert!(compact.contains("settings/default.yml:1-1"), "{compact}");
        assert!(!compact.contains("retry_policy") && !compact.contains("region.toml"));

        for cap in [1, 3, 45, 120, 250] {
            let reply = live_candidates(root, question, &[], 1, Some(10), cap).unwrap();
            assert!(reply.len() <= cap, "budget {cap}: {reply}");
        }
    }
}
