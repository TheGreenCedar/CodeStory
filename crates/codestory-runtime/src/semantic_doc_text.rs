use codestory_contracts::graph::NodeKind;
use std::collections::HashSet;

use crate::symbol_query::symbol_query_tokens;

pub(crate) fn semantic_doc_language_from_path(path: Option<&str>) -> Option<&'static str> {
    codestory_contracts::language_support::language_name_for_path(path)
}

pub(crate) fn semantic_symbol_role_aliases(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::MODULE => "module namespace package container",
        NodeKind::NAMESPACE => "namespace module package container",
        NodeKind::PACKAGE => "package module namespace container",
        NodeKind::FILE => "file source module unit",
        NodeKind::STRUCT => "struct record data type model",
        NodeKind::CLASS => "class object type model",
        NodeKind::INTERFACE => "interface protocol contract abstraction",
        NodeKind::ANNOTATION => "annotation decorator attribute metadata",
        NodeKind::UNION => "union variant data type",
        NodeKind::ENUM => "enum enumeration choices variants",
        NodeKind::TYPEDEF => "typedef type alias named type",
        NodeKind::TYPE_PARAMETER => "type parameter generic parameter",
        NodeKind::BUILTIN_TYPE => "builtin type primitive",
        NodeKind::FUNCTION => "function callable routine procedure operation",
        NodeKind::METHOD => "method member function object behavior callable routine operation",
        NodeKind::MACRO => "macro compile time expansion preprocessor metaprogramming",
        NodeKind::GLOBAL_VARIABLE => "global variable shared state value",
        NodeKind::FIELD => "field property member attribute",
        NodeKind::VARIABLE => "variable local value binding",
        NodeKind::CONSTANT => "constant fixed value configuration",
        NodeKind::ENUM_CONSTANT => "enum constant variant case",
        NodeKind::UNKNOWN => "unknown symbol unresolved placeholder",
    }
}

#[derive(Debug, Default)]
pub(crate) struct SemanticSymbolAliases {
    pub(crate) name_aliases: Vec<String>,
    pub(crate) terminal_alias: Option<String>,
    pub(crate) owner_aliases: Vec<String>,
}

pub(crate) fn semantic_symbol_aliases(
    display_name: &str,
    qualified_name: Option<&str>,
) -> SemanticSymbolAliases {
    let mut aliases = SemanticSymbolAliases::default();
    let mut seen_names = HashSet::new();
    let mut seen_owners = HashSet::new();
    let candidates = [Some(display_name), qualified_name];

    for candidate in candidates.into_iter().flatten() {
        if let Some(alias) = normalized_symbol_alias(candidate) {
            push_unique_alias(&mut aliases.name_aliases, &mut seen_names, alias);
        }
        for expanded_alias in expanded_symbol_aliases(candidate) {
            push_unique_alias(&mut aliases.name_aliases, &mut seen_names, expanded_alias);
        }
        if let Some(terminal) = terminal_symbol_part(candidate)
            && let Some(alias) = normalized_symbol_alias(terminal)
        {
            if aliases.terminal_alias.is_none() {
                aliases.terminal_alias = Some(alias.clone());
            }
            push_unique_alias(&mut aliases.name_aliases, &mut seen_names, alias);
            for expanded_alias in expanded_symbol_aliases(terminal) {
                push_unique_alias(&mut aliases.name_aliases, &mut seen_names, expanded_alias);
            }
        }

        let owner_parts = owner_symbol_parts(candidate);
        if let Some(owner) = owner_parts.last() {
            push_unique_alias(
                &mut aliases.owner_aliases,
                &mut seen_owners,
                (*owner).to_string(),
            );
            if let Some(alias) = normalized_symbol_alias(owner) {
                push_unique_alias(&mut aliases.owner_aliases, &mut seen_owners, alias);
            }
            for expanded_alias in expanded_symbol_aliases(owner) {
                push_unique_alias(&mut aliases.owner_aliases, &mut seen_owners, expanded_alias);
            }
        }
        if owner_parts.len() > 1 {
            let full_owner = owner_parts.join(" ");
            if let Some(alias) = normalized_symbol_alias(&full_owner) {
                push_unique_alias(&mut aliases.owner_aliases, &mut seen_owners, alias);
            }
            for expanded_alias in expanded_symbol_aliases(&full_owner) {
                push_unique_alias(&mut aliases.owner_aliases, &mut seen_owners, expanded_alias);
            }
        }
    }

    aliases
}

pub(crate) fn semantic_path_aliases(file_path: Option<&str>, limit: usize) -> Vec<String> {
    let Some(path) = file_path else {
        return Vec::new();
    };
    let extension = path.rsplit(['/', '\\']).next().and_then(|file_name| {
        file_name
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
    });
    let mut aliases = Vec::new();
    let mut seen = HashSet::new();
    for component in path
        .split(['/', '\\', '.'])
        .map(str::trim)
        .filter(|component| !component.is_empty())
    {
        if extension
            .as_deref()
            .is_some_and(|ext| component.eq_ignore_ascii_case(ext))
        {
            continue;
        }
        push_unique_alias(&mut aliases, &mut seen, component.to_string());
        if let Some(normalized) = normalized_symbol_alias(component)
            && normalized != component
        {
            push_unique_alias(&mut aliases, &mut seen, normalized);
        }
        if aliases.len() >= limit {
            aliases.truncate(limit);
            break;
        }
    }
    aliases
}

fn push_unique_alias(aliases: &mut Vec<String>, seen: &mut HashSet<String>, alias: String) {
    let alias = alias.trim();
    if alias.is_empty() || !seen.insert(alias.to_ascii_lowercase()) {
        return;
    }
    aliases.push(alias.to_string());
}

fn normalized_symbol_alias(value: &str) -> Option<String> {
    let alias = symbol_query_tokens(value).join(" ");
    (!alias.is_empty()).then_some(alias)
}

fn expanded_symbol_aliases(value: &str) -> Vec<String> {
    let tokens = symbol_query_tokens(value);
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut aliases = Vec::new();
    let mut seen = HashSet::new();

    if tokens.iter().any(|token| token == "cxx") {
        push_expanded_tokens(&tokens, &[("cxx", &["c++"][..])], &mut aliases, &mut seen);
        push_expanded_tokens(&tokens, &[("cxx", &["cpp"][..])], &mut aliases, &mut seen);
    }
    if tokens.iter().any(|token| token == "cdb") {
        let replacements = [
            ("cdb", &["compilation", "database"][..]),
            ("cdb", &["compile", "commands"][..]),
            ("cdb", &["compile", "commands", "json"][..]),
        ];
        for cdb_replacement in replacements {
            push_expanded_tokens(&tokens, &[cdb_replacement], &mut aliases, &mut seen);
            push_expanded_tokens(
                &tokens,
                &[("cxx", &["c++"][..]), cdb_replacement],
                &mut aliases,
                &mut seen,
            );
            push_expanded_tokens(
                &tokens,
                &[("cxx", &["cpp"][..]), cdb_replacement],
                &mut aliases,
                &mut seen,
            );
        }
    }

    aliases
}

fn push_expanded_tokens(
    tokens: &[String],
    replacements: &[(&str, &[&str])],
    aliases: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    let expanded = tokens
        .iter()
        .flat_map(|token| {
            replacements
                .iter()
                .find_map(|(needle, replacement)| {
                    (token == needle).then(|| replacement.iter().copied())
                })
                .into_iter()
                .flatten()
                .chain(
                    (!replacements.iter().any(|(needle, _)| token == needle))
                        .then_some(token.as_str()),
                )
        })
        .collect::<Vec<_>>();
    push_unique_alias(aliases, seen, expanded.join(" "));
}

fn namespace_parts(value: &str) -> Vec<&str> {
    value
        .split("::")
        .flat_map(|part| part.split(['.', '#']))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

fn terminal_symbol_part(value: &str) -> Option<&str> {
    namespace_parts(value).into_iter().last()
}

fn owner_symbol_parts(value: &str) -> Vec<&str> {
    let mut parts = namespace_parts(value);
    if parts.len() <= 1 {
        return Vec::new();
    }
    parts.pop();
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_from_path_covers_supported_extensions() {
        let cases = [
            ("main.c", Some("c")),
            ("main.cpp", Some("cpp")),
            ("Main.java", Some("java")),
            ("main.js", Some("javascript")),
            ("main.py", Some("python")),
            ("main.rs", Some("rust")),
            ("main.ts", Some("typescript")),
            ("main.go", Some("go")),
            ("main.rb", Some("ruby")),
            ("main.php", Some("php")),
            ("Program.cs", Some("csharp")),
            ("View.cshtml", None),
            ("Main.kt", Some("kotlin")),
            ("Main.swift", Some("swift")),
            ("main.dart", Some("dart")),
            ("script.sh", Some("bash")),
            ("index.html", Some("html")),
            ("style.css", Some("css")),
            ("schema.sql", Some("sql")),
            ("README.md", None),
        ];

        for (path, language) in cases {
            assert_eq!(semantic_doc_language_from_path(Some(path)), language);
        }
    }

    #[test]
    fn symbol_aliases_split_namespaces_camel_snake_and_acronyms() {
        let aliases = semantic_symbol_aliases(
            "HTTPServer::parseURL2Value",
            Some("crate::net_io::HTTPServer::parseURL2Value"),
        );

        assert_eq!(
            aliases.name_aliases,
            vec![
                "http server parse url 2 value",
                "parse url 2 value",
                "crate net io http server parse url 2 value"
            ]
        );
        assert_eq!(aliases.terminal_alias.as_deref(), Some("parse url 2 value"));
        assert!(aliases.owner_aliases.contains(&"HTTPServer".to_string()));
        assert!(aliases.owner_aliases.contains(&"http server".to_string()));
    }

    #[test]
    fn symbol_aliases_expand_cpp_cdb_owner_acronyms() {
        let aliases = semantic_symbol_aliases(
            "SourceGroupCxxCdb::getIndexerCommandProvider",
            Some("sourcetrail::SourceGroupCxxCdb::getIndexerCommandProvider"),
        );

        assert!(
            aliases
                .owner_aliases
                .contains(&"source group c++ compilation database".to_string())
        );
        assert!(
            aliases
                .owner_aliases
                .contains(&"source group c++ compile commands json".to_string())
        );
    }

    #[test]
    fn symbol_aliases_expand_cpp_cdb_terminal_acronyms() {
        let aliases = semantic_symbol_aliases("SourceGroupCxxCdb", Some("SourceGroupCxxCdb"));

        assert!(
            aliases
                .name_aliases
                .contains(&"source group c++ compilation database".to_string())
        );
        assert!(
            aliases
                .name_aliases
                .contains(&"source group c++ compile commands json".to_string())
        );
    }

    #[test]
    fn path_aliases_keep_raw_and_normalized_components_without_extension() {
        assert_eq!(
            semantic_path_aliases(Some("crates/codestory-runtime/src/lib.rs"), 8),
            vec![
                "crates",
                "codestory-runtime",
                "codestory runtime",
                "src",
                "lib"
            ]
        );
    }
}
