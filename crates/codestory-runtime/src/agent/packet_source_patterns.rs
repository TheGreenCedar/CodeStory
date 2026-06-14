use crate::agent::packet_scoring::normalize_identifier;

pub(crate) fn packet_source_has_all(source: &str, terms: &[&str]) -> bool {
    let lower = source.to_ascii_lowercase();
    terms
        .iter()
        .all(|term| lower.contains(&term.to_ascii_lowercase()))
}

pub(crate) fn packet_source_has_any(source: &str, terms: &[&str]) -> bool {
    let lower = source.to_ascii_lowercase();
    terms
        .iter()
        .any(|term| lower.contains(&term.to_ascii_lowercase()))
}

pub(crate) fn packet_source_identifier_with_words(source: &str, words: &[&str]) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    for token in source.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let normalized = normalize_identifier(token);
        if words.iter().all(|word| normalized.contains(word)) {
            return Some(token.to_string());
        }
    }
    None
}

pub(crate) fn packet_source_identifier_with_words_shortest(
    source: &str,
    words: &[&str],
) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    let mut best: Option<String> = None;
    for token in source.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let normalized = normalize_identifier(token);
        if !words.iter().all(|word| normalized.contains(word)) {
            continue;
        }
        let replace = best
            .as_ref()
            .map(|existing| token.len() < existing.len())
            .unwrap_or(true);
        if replace {
            best = Some(token.to_string());
        }
    }
    best
}

pub(crate) fn packet_source_identifier_exact(source: &str, word: &str) -> Option<String> {
    for token in source.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')) {
        let token = token.trim();
        if token.eq_ignore_ascii_case(word) {
            return Some(token.to_string());
        }
    }
    None
}

pub(crate) fn packet_source_identifier_ending_with(
    source: &str,
    suffix: &str,
    excluded: &str,
) -> Option<String> {
    for token in source.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')) {
        let token = token.trim();
        if token.is_empty() || token.eq_ignore_ascii_case(excluded) {
            continue;
        }
        if token.ends_with(suffix) {
            return Some(token.to_string());
        }
    }
    None
}

pub(crate) fn packet_source_constructed_type(source: &str) -> Option<String> {
    let bytes = source.as_bytes();
    let needle = b"new ";
    let mut index = 0;
    while index + needle.len() < bytes.len() {
        if &bytes[index..index + needle.len()] != needle {
            index += 1;
            continue;
        }
        let mut start = index + needle.len();
        while start < bytes.len() && bytes[start].is_ascii_whitespace() {
            start += 1;
        }
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            end += 1;
        }
        if end > start {
            let value = &source[start..end];
            if value
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_uppercase())
            {
                return Some(value.to_string());
            }
        }
        index = end.saturating_add(1);
    }
    None
}

pub(crate) fn packet_display_owner(display: &str) -> Option<String> {
    let owner = display
        .split(['.', ':', '#', '_'])
        .find(|part| {
            part.chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_uppercase())
        })?
        .trim();
    if owner.is_empty() {
        None
    } else {
        Some(owner.to_string())
    }
}

pub(crate) fn packet_sql_create_table_names(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in source.lines() {
        if let Some(name) = packet_sql_identifier_after(line, "create table")
            && !names.iter().any(|existing| existing == &name)
        {
            names.push(name);
        }
        if names.len() >= 12 {
            break;
        }
    }
    names
}

pub(crate) fn packet_sql_foreign_key_claims(source: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut current_table: Option<String> = None;
    for line in source.lines() {
        if let Some(table) = packet_sql_identifier_after(line, "create table") {
            current_table = Some(table);
        }
        let normalized = line.to_ascii_lowercase();
        if !normalized.contains("foreign key") || !normalized.contains("references") {
            continue;
        }
        let Some(source_table) = current_table.clone() else {
            continue;
        };
        let Some(local_key) = packet_sql_identifier_between(line, "foreign key", "references")
        else {
            continue;
        };
        let Some(target_table) = packet_sql_identifier_after(line, "references") else {
            continue;
        };
        if !links
            .iter()
            .any(|(existing_source, existing_target, existing_key)| {
                existing_source == &source_table
                    && existing_target == &target_table
                    && existing_key == &local_key
            })
        {
            links.push((source_table, target_table, local_key));
        }
        if links.len() >= 18 {
            break;
        }
    }

    let mut claims = Vec::new();
    for (source_table, target_table, local_key) in &links {
        claims.push(format!(
            "{source_table} rows reference {target_table} rows through {local_key}."
        ));
    }

    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    for (source_table, target_table, _) in links {
        if let Some((_, targets)) = grouped
            .iter_mut()
            .find(|(existing_source, _)| existing_source == &source_table)
        {
            if !targets.iter().any(|existing| existing == &target_table) {
                targets.push(target_table);
            }
        } else {
            grouped.push((source_table, vec![target_table]));
        }
    }
    for (source_table, targets) in grouped {
        if targets.len() < 2 {
            continue;
        }
        let claim = format!(
            "{source_table} rows reference {} rows.",
            packet_human_join(&targets)
        );
        if !claims.iter().any(|existing| existing == &claim) {
            claims.push(claim);
        }
    }

    claims
}

fn packet_sql_identifier_between(line: &str, start: &str, end: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let start_at = lower.find(start)? + start.len();
    let end_at = lower[start_at..].find(end)? + start_at;
    packet_first_sql_identifier(&line[start_at..end_at])
}

pub(crate) fn packet_sql_identifier_after(line: &str, needle: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let at = lower.find(needle)? + needle.len();
    if needle == "create table"
        && lower[at..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
    {
        return None;
    }
    let mut rest = line[at..].trim_start();
    for prefix in ["if not exists", "only"] {
        if rest.to_ascii_lowercase().starts_with(prefix) {
            rest = rest[prefix.len()..].trim_start();
        }
    }
    packet_first_sql_identifier(rest)
}

fn packet_first_sql_identifier(input: &str) -> Option<String> {
    let mut token = String::new();
    let mut in_identifier = false;
    let mut quote: Option<char> = None;
    for ch in input.chars() {
        if !in_identifier {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '"' | '\'' | '`' | '[') {
                in_identifier = true;
                quote = match ch {
                    '"' | '\'' | '`' => Some(ch),
                    '[' => Some(']'),
                    _ => None,
                };
                if quote.is_none() {
                    token.push(ch);
                }
            }
            continue;
        }
        if quote.is_some_and(|end| ch == end) {
            break;
        }
        if quote.is_none() && !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '$')) {
            break;
        }
        token.push(ch);
    }
    let token = token
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | '[' | ']' | '(' | ')'))
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | '[' | ']'))
        .trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

pub(crate) fn packet_human_join(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        _ => {
            let mut parts = items.to_vec();
            let last = parts.pop().unwrap_or_default();
            format!("{}, and {last}", parts.join(", "))
        }
    }
}
