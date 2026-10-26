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
            .is_some_and(|x| x == "py" || x == "rs")
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
    let mut accepted = Vec::new();
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
            accepted.push(record);
        }
    }
    if observations.is_empty() {
        return None;
    }
    Some(SourceContextRequest {
        project: project.to_str()?.to_owned(),
        cwd: cwd?,
        observations,
        query: captured_query(&accepted),
    })
}

fn captured_query(
    records: &[&Record],
) -> Option<codestory_contracts::source_context::SourceContextQuery> {
    let mut single = None;
    for record in records {
        if record.config_path_absent != Some(true) {
            return None;
        }
        let pattern = actual_pattern(&record.argv)?;
        if single.as_ref().is_some_and(|old| old != &pattern) {
            return None;
        }
        single = Some(pattern);
    }
    Some(codestory_contracts::source_context::SourceContextQuery { pattern: single? })
}

/// Only known argv roles for ordinary numbered, case-sensitive regex rows.
/// Pattern grammar/feature admission remains runtime-owned. Any accepted record
/// with unknown roles disables features for the entire request.
fn actual_pattern(args: &[String]) -> Option<String> {
    let mut pattern = None;
    let mut positional = false;
    let mut after_options = false;
    let mut i = 0;
    while i < args.len() {
        let value = &args[i];
        if !after_options && value == "--" {
            after_options = true;
        } else if !after_options && value.starts_with('-') {
            match value.as_str() {
                "--line-number" | "--with-filename" | "--no-heading" | "--hidden"
                | "--case-sensitive" | "--color=never" => {}
                "--color" => {
                    i += 1;
                    if args.get(i).map(String::as_str) != Some("never") {
                        return None;
                    }
                }
                "-e" | "--regexp" => {
                    i += 1;
                    if positional || pattern.is_some() {
                        return None;
                    }
                    pattern = Some(args.get(i)?.clone());
                }
                "-g" | "--glob" | "--iglob" | "-t" | "--type" | "-T" | "--type-not" | "-m"
                | "--max-count" | "--sort" | "--sortr" => {
                    i += 1;
                    if args.get(i)?.starts_with('-') {
                        return None;
                    }
                }
                _ if value.len() > 1
                    && !value.starts_with("--")
                    && value[1..].chars().all(|c| "nHs".contains(c)) => {}
                _ => return None,
            }
        } else if pattern.is_none() {
            positional = true;
            pattern = Some(value.clone());
        }
        i += 1;
    }
    pattern
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(args: &[&str], output: &str, known: Option<bool>) -> Record {
        Record {
            argv: args.iter().map(|s| s.to_string()).collect(),
            cwd: "/project".into(),
            stdout: output.as_bytes().to_vec(),
            code: 0,
            overflow: false,
            config_path_absent: known,
        }
    }

    #[test]
    fn query_metadata_comes_only_from_accepted_numbered_records_with_known_config() {
        let inventory = record(&["--files", "-g", "*.rs"], "source.rs\n", Some(true));
        let eligible = record(
            &["-Hn", "--color=never", "red|blue", "source.rs"],
            "source.rs:2:    red();\n",
            Some(true),
        );
        let request = request(Path::new("/project"), &[inventory, eligible]).unwrap();
        assert_eq!(request.observations.len(), 1);
        assert_eq!(
            request
                .query
                .expect("inventory must not poison the single actual query")
                .pattern,
            "red|blue"
        );
        let multi_operand = record(
            &["-n", "red|blue", "src", "tests/observe.rs", "Cargo.toml"],
            "src/module.rs:2:    red();\ntests/observe.rs:3:    blue();\n",
            Some(true),
        );
        let multi = super::request(
            Path::new("/project"),
            &[
                record(&["--files"], "src/module.rs\nCargo.toml\n", Some(true)),
                multi_operand,
            ],
        )
        .unwrap();
        assert_eq!(multi.observations.len(), 2);
        assert_eq!(multi.observations[0].path, "src/module.rs");
        assert_eq!(multi.observations[1].path, "tests/observe.rs");
        assert_eq!(
            multi
                .query
                .expect("numbered directory plus multiple operands is an eligible actual query")
                .pattern,
            "red|blue"
        );
    }

    #[test]
    fn unknown_config_modes_and_mixed_queries_keep_existing_ordinary_rows() {
        for known in [None, Some(false)] {
            let response = request(
                Path::new("/project"),
                &[record(
                    &["-Hn", "red|blue", "source.rs"],
                    "source.rs:2:red\n",
                    known,
                )],
            )
            .unwrap();
            assert_eq!(response.observations.len(), 1);
            assert!(response.query.is_none());
        }
        for flag in [
            "-i",
            "-S",
            "-F",
            "-w",
            "-x",
            "--ignore-case",
            "--smart-case",
            "--fixed-strings",
            "--word-regexp",
            "--line-regexp",
        ] {
            let response = request(
                Path::new("/project"),
                &[record(
                    &["-Hn", flag, "red|blue", "source.rs"],
                    "source.rs:2:red\n",
                    Some(true),
                )],
            )
            .unwrap();
            assert_eq!(response.observations.len(), 1);
            assert!(response.query.is_none(), "unsupported mode {flag}");
        }
        for (pattern, known) in [("green|yellow", Some(true)), ("red|blue", None)] {
            let response = request(
                Path::new("/project"),
                &[
                    record(
                        &["-Hn", "red|blue", "source.rs"],
                        "source.rs:2:red\n",
                        Some(true),
                    ),
                    record(&["-Hn", pattern, "source.rs"], "source.rs:3:green\n", known),
                ],
            )
            .unwrap();
            assert_eq!(response.observations.len(), 2);
            assert!(response.query.is_none());
        }
        let mut incomplete = record(
            &["-Hn", "red|blue", "source.rs"],
            "source.rs:2:red",
            Some(true),
        );
        assert!(request(Path::new("/project"), &[incomplete]).is_none());
        incomplete = record(
            &["-Hn", "red|blue", "source.rs"],
            "source.rs:2:red\n",
            Some(true),
        );
        incomplete.overflow = true;
        assert!(request(Path::new("/project"), &[incomplete]).is_none());
        assert!(
            request(
                Path::new("/project"),
                &[record(
                    &["-Hn", "--pcre2", "red|blue", "source.rs"],
                    "source.rs:2:red\n",
                    Some(true)
                )]
            )
            .is_none(),
            "an unsupported old row mode must retain its ordinary no-context behavior"
        );
    }
}
