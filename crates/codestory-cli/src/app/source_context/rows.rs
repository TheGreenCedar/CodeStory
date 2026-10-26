//! Conservative actual-argv and complete-row grammar; never parse a shell script.
use super::transport::Record;
use codestory_contracts::source_context::{SourceContextRequest, SourceRowObservation};
use std::path::Path;

fn single_file(args: &[String]) -> Option<Option<String>> {
    let mut numbered = false;
    let mut forced = false;
    let mut patterns = 0usize;
    let mut operands = Vec::new();
    let mut roles = true;
    let mut end = false;
    let mut i = 0;
    while i < args.len() {
        let value = &args[i];
        if !end && value == "--" {
            end = true;
            i += 1;
            continue;
        }
        if !end && value.starts_with('-') {
            match value.as_str() {
                "--line-number" => numbered = true,
                "--with-filename" => forced = true,
                "--no-heading" | "--hidden" | "--ignore-case" | "--case-sensitive"
                | "--smart-case" | "--fixed-strings" | "--word-regexp" | "--line-regexp"
                | "--color=never" => {}
                "--color" => {
                    i += 1;
                    if args.get(i).map(String::as_str) != Some("never") {
                        return None;
                    }
                }
                "-e" | "--regexp" | "-g" | "--glob" | "--iglob" | "-t" | "--type" | "-T"
                | "--type-not" | "-m" | "--max-count" | "--sort" | "--sortr" => {
                    i += 1;
                    let operand = args.get(i)?;
                    if operand.starts_with('-') {
                        roles = false;
                    }
                    if value == "-e" || value == "--regexp" {
                        if !operands.is_empty() {
                            roles = false;
                        }
                        patterns += 1;
                    }
                }
                _ if value.starts_with('-')
                    && !value.starts_with("--")
                    && value.len() > 1
                    && value[1..].chars().all(|c| "nHisSFwx".contains(c)) =>
                {
                    numbered |= value.contains('n');
                    forced |= value.contains('H');
                }
                _ => return None,
            }
        } else {
            operands.push(value.clone());
        }
        i += 1;
    }
    if !numbered || (patterns == 0 && operands.is_empty()) {
        return None;
    }
    if patterns == 0 && !operands.is_empty() {
        operands.remove(0);
    }
    let inferred = if roles
        && !forced
        && operands.len() == 1
        && operands[0] != "-"
        && !operands[0].starts_with('=')
        && Path::new(&operands[0])
            .extension()
            .is_some_and(|x| x == "py")
    {
        Some(operands.remove(0))
    } else {
        None
    };
    Some(inferred)
}

pub(super) fn request(project: &Path, records: &[Record]) -> Option<SourceContextRequest> {
    let mut cwd = None;
    let mut observations = Vec::new();
    for record in records {
        if record.code != 0 || record.overflow {
            continue;
        }
        let Some(single) = single_file(&record.argv) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&record.stdout) else {
            continue;
        };
        if text.is_empty() || !text.ends_with('\n') || text.contains(['\0', '\u{1b}']) {
            continue;
        }
        if let Some(first) = &cwd {
            if *first != record.cwd {
                continue;
            }
        } else {
            cwd = Some(record.cwd.clone());
        }
        let mut parsed = Vec::new();
        let mut grammar = None;
        let mut valid = true;
        for row in text.split_terminator('\n') {
            let Some((prefix, rest)) = row.split_once(':') else {
                valid = false;
                break;
            };
            let numbered = !prefix.is_empty()
                && prefix.bytes().all(|b| b.is_ascii_digit())
                && !prefix.starts_with('0');
            if grammar.is_some_and(|old| old != numbered) {
                valid = false;
                break;
            }
            grammar = Some(numbered);
            let (path, line, body) = if numbered {
                let Some(path) = &single else {
                    valid = false;
                    break;
                };
                if path.contains([':', '\n', '\r']) {
                    valid = false;
                    break;
                }
                (path.as_str(), prefix, rest)
            } else {
                if prefix.is_empty() || prefix.contains('\r') {
                    valid = false;
                    break;
                }
                let Some((line, body)) = rest.split_once(':') else {
                    valid = false;
                    break;
                };
                (prefix, line, body)
            };
            let Some(line) = line.parse::<usize>().ok().filter(|line| *line > 0) else {
                valid = false;
                break;
            };
            parsed.push(SourceRowObservation {
                path: path.to_owned(),
                physical_line: line,
                text: body.to_owned(),
            });
        }
        if valid {
            observations.extend(parsed);
        }
    }
    if observations.is_empty() {
        return None;
    }
    Some(SourceContextRequest {
        project: project.to_str()?.to_owned(),
        cwd: cwd?,
        observations,
    })
}
