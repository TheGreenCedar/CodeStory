use serde::{Deserialize, Serialize};

pub const QUERY_INTENT_POLICY_VERSION: &str = "multi_label_v2";

/// High-level query shape used by the planner (repo-agnostic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryShape {
    SymbolLike,
    PathLike,
    NaturalLanguage,
    Mixed,
}

/// How the query expects retrieved evidence to be used.
///
/// This stays internal to retrieval planning. It is deliberately independent
/// from the legacy `QueryShape`: one mixed question can still carry symbol,
/// path, relationship, and flow intent at the same time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryLookupMode {
    Definition,
    Relation,
    OrderedFlow,
    #[default]
    Explanation,
}

/// Independent query intents used to plan complementary retrieval lanes.
///
/// A prompt can name a symbol and a path while also asking for a relationship;
/// these labels are deliberately not mutually exclusive.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryIntent {
    pub symbol: bool,
    pub path: bool,
    pub natural_language: bool,
    pub relationship: bool,
    pub standalone_symbol: bool,
    pub standalone_path: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exact_symbols: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub concepts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ordered_flow_stages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_roles: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub structural_kinds: Vec<String>,
    #[serde(default)]
    pub lookup_mode: QueryLookupMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryFeatures {
    pub raw_query: String,
    #[serde(default = "current_query_intent_policy")]
    pub intent_policy: String,
    pub shape: QueryShape,
    #[serde(default)]
    pub intent: QueryIntent,
    pub token_count: usize,
    pub has_path_separators: bool,
    pub has_camel_case_token: bool,
    pub has_snake_case_token: bool,
    pub looks_like_qualified_symbol: bool,
}

pub fn classify_query(query: &str) -> QueryFeatures {
    let trimmed = query.trim();
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    let token_count = tokens.len().max(1);

    let has_path_separators = trimmed.contains('/') || trimmed.contains('\\');
    let standalone_token = token_count == 1;
    let has_camel_case_token = tokens.iter().any(|token| {
        !looks_like_path_token(token, standalone_token) && has_internal_camel_hump(token)
    });
    let has_snake_case_token = tokens.iter().any(|token| {
        !looks_like_path_token(token, standalone_token)
            && token.contains('_')
            && token
                .chars()
                .filter(|c| c.is_ascii_alphabetic())
                .all(|c| c.is_ascii_lowercase() || c == '_')
    });
    let looks_like_qualified_symbol = tokens
        .iter()
        .any(|token| !looks_like_path_token(token, standalone_token) && is_qualified_symbol(token));

    let path_like = tokens
        .iter()
        .any(|token| looks_like_path_token(token, token_count == 1))
        || (token_count == 1 && has_supported_file_extension(trimmed));
    let symbol_like = looks_like_qualified_symbol
        || has_camel_case_token
        || has_snake_case_token
        || (token_count == 1 && trimmed.chars().all(|c| c.is_alphanumeric() || c == '_'));

    let nl_like = (token_count == 1 && !path_like && !symbol_like)
        || token_count >= 3
        || trimmed.split_whitespace().any(|word| {
            matches!(
                word.to_ascii_lowercase().as_str(),
                "how" | "what" | "where" | "why" | "when" | "explain" | "find"
            )
        });

    let relationship = tokens.iter().any(|word| {
        matches!(
            normalized_word(word).as_str(),
            "call"
                | "calls"
                | "called"
                | "caller"
                | "callers"
                | "depend"
                | "depends"
                | "dependency"
                | "dependencies"
                | "flow"
                | "flows"
                | "through"
                | "use"
                | "uses"
                | "used"
                | "owner"
                | "owns"
        )
    });
    let exact_symbols = unique_labels(tokens.iter().filter_map(|token| {
        let token = trim_query_token(token);
        (!looks_like_path_token(token, standalone_token)
            && (is_qualified_symbol(token)
                || has_internal_camel_hump(token)
                || token.contains('_')))
        .then(|| token.to_string())
    }));
    let paths = unique_labels(tokens.iter().filter_map(|token| {
        let token = trim_query_token(token);
        looks_like_path_token(token, token_count == 1).then(|| token.replace('\\', "/"))
    }));
    let normalized_tokens = tokens
        .iter()
        .flat_map(|token| intent_words(token))
        .collect::<Vec<_>>();
    let relations = unique_labels(normalized_tokens.iter().filter_map(|token| {
        matches!(
            token.as_str(),
            "call"
                | "calls"
                | "called"
                | "caller"
                | "callers"
                | "depend"
                | "depends"
                | "dependency"
                | "dependencies"
                | "flow"
                | "flows"
                | "through"
                | "use"
                | "uses"
                | "used"
                | "owner"
                | "owns"
                | "route"
                | "routes"
                | "dispatch"
                | "handoff"
        )
        .then(|| token.clone())
    }));
    let ordered_flow_stages = unique_labels(normalized_tokens.iter().filter_map(|token| {
        matches!(
            token.as_str(),
            "before"
                | "after"
                | "then"
                | "first"
                | "next"
                | "finally"
                | "flow"
                | "pipeline"
                | "stages"
                | "handoff"
        )
        .then(|| token.clone())
    }));
    let evidence_roles = unique_labels(normalized_tokens.iter().filter_map(|token| {
        matches!(
            token.as_str(),
            "adapter"
                | "controller"
                | "dispatch"
                | "dispatcher"
                | "driver"
                | "entrypoint"
                | "executor"
                | "handler"
                | "indexer"
                | "parser"
                | "reader"
                | "retrieval"
                | "router"
                | "runner"
                | "search"
                | "service"
                | "storage"
                | "store"
                | "worker"
                | "writer"
        )
        .then(|| token.clone())
    }));
    let structural_kinds = unique_labels(normalized_tokens.iter().filter_map(|token| {
        matches!(
            token.as_str(),
            "class"
                | "enum"
                | "file"
                | "function"
                | "interface"
                | "macro"
                | "method"
                | "module"
                | "namespace"
                | "route"
                | "struct"
                | "trait"
        )
        .then(|| token.clone())
    }));
    let concepts = unique_labels(normalized_tokens.iter().filter_map(|token| {
        (token.len() >= 3
            && !QUERY_STOPWORDS.contains(&token.as_str())
            && !relations.iter().any(|relation| relation == token)
            && !evidence_roles.iter().any(|role| role == token)
            && !structural_kinds.iter().any(|kind| kind == token))
        .then(|| token.clone())
    }));
    let lookup_mode = if !ordered_flow_stages.is_empty() && relationship {
        QueryLookupMode::OrderedFlow
    } else if relationship {
        QueryLookupMode::Relation
    } else if (symbol_like || path_like) && !nl_like {
        QueryLookupMode::Definition
    } else {
        QueryLookupMode::Explanation
    };
    let intent = QueryIntent {
        symbol: symbol_like,
        path: path_like,
        natural_language: nl_like,
        relationship,
        standalone_symbol: symbol_like && !path_like && !nl_like && token_count == 1,
        standalone_path: path_like && !nl_like && token_count == 1,
        exact_symbols,
        paths,
        concepts,
        relations,
        ordered_flow_stages,
        evidence_roles,
        structural_kinds,
        lookup_mode,
    };

    let shape = if nl_like && (path_like || symbol_like) {
        QueryShape::Mixed
    } else if path_like {
        QueryShape::PathLike
    } else if symbol_like && !nl_like {
        QueryShape::SymbolLike
    } else if nl_like && !symbol_like {
        QueryShape::NaturalLanguage
    } else if nl_like {
        QueryShape::Mixed
    } else if symbol_like {
        QueryShape::SymbolLike
    } else {
        QueryShape::NaturalLanguage
    };

    QueryFeatures {
        raw_query: trimmed.to_string(),
        intent_policy: QUERY_INTENT_POLICY_VERSION.into(),
        shape,
        intent,
        token_count,
        has_path_separators,
        has_camel_case_token,
        has_snake_case_token,
        looks_like_qualified_symbol,
    }
}

fn current_query_intent_policy() -> String {
    QUERY_INTENT_POLICY_VERSION.into()
}

const QUERY_STOPWORDS: &[&str] = &[
    "and", "are", "does", "explain", "find", "for", "from", "how", "into", "the", "this", "what",
    "when", "where", "which", "why", "with",
];

fn trim_query_token(token: &str) -> &str {
    token.trim_matches(|character: char| {
        matches!(
            character,
            '`' | '\'' | '"' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}'
        )
    })
}

fn unique_labels(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut values = values.into_iter().collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn normalized_word(word: &str) -> String {
    word.trim_matches(|character: char| !character.is_alphanumeric() && character != '_')
        .to_ascii_lowercase()
}

fn intent_words(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    for segment in value.split(|character: char| !character.is_alphanumeric()) {
        if segment.is_empty() {
            continue;
        }
        let mut start = 0;
        let characters = segment.char_indices().collect::<Vec<_>>();
        for window in characters.windows(2) {
            let (left_index, left) = window[0];
            let (right_index, right) = window[1];
            if (left.is_lowercase() || left.is_ascii_digit()) && right.is_uppercase() {
                if left_index + left.len_utf8() > start {
                    words.push(segment[start..right_index].to_ascii_lowercase());
                }
                start = right_index;
            }
        }
        if start < segment.len() {
            words.push(segment[start..].to_ascii_lowercase());
        }
    }
    words
}

fn looks_like_path_token(token: &str, _standalone: bool) -> bool {
    let token = token.trim_matches(|character: char| {
        matches!(
            character,
            '`' | '\'' | '"' | ',' | ';' | ':' | '(' | ')' | '[' | ']'
        )
    });
    if !token.contains('/') && !token.contains('\\') {
        return false;
    }
    if token.starts_with('/')
        || token.starts_with("./")
        || token.starts_with("../")
        || token.get(1..3).is_some_and(|prefix| prefix == ":\\")
    {
        return true;
    }
    let normalized = token.replace('\\', "/");
    let final_component = normalized.rsplit('/').next().unwrap_or(&normalized);
    if has_supported_file_extension(final_component) {
        return true;
    }
    normalized
        .chars()
        .filter(|character| *character == '/')
        .count()
        >= 2
}

/// A camel hump is an interior lowercase-to-uppercase transition.
///
/// Any capitalised word satisfies "has an uppercase and a lowercase letter", so
/// that weaker test reads a sentence-leading `How` or `Explain` as a symbol and
/// demotes the natural-language profile that owns broad recall.
fn has_internal_camel_hump(token: &str) -> bool {
    token
        .chars()
        .zip(token.chars().skip(1))
        .any(|(previous, current)| previous.is_lowercase() && current.is_uppercase())
}

/// A qualifier only counts between two name characters.
///
/// Sentence punctuation puts a trailing `.` on ordinary prose, which otherwise
/// reads as a qualified symbol and misroutes the whole query.
fn is_qualified_symbol(token: &str) -> bool {
    let characters: Vec<char> = token.chars().collect();
    characters
        .iter()
        .enumerate()
        .skip(1)
        .take(characters.len().saturating_sub(2))
        .any(|(index, current)| {
            let separator_len = match (*current, characters.get(index + 1)) {
                ('.', _) => 1,
                (':', Some(':')) => 2,
                _ => return false,
            };
            let Some(following) = characters.get(index + separator_len) else {
                return false;
            };
            is_name_character(characters[index - 1]) && is_name_character(*following)
        })
}

fn is_name_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// A bare filename is path-like only when the registry claims its extension.
///
/// Spelling a few extensions inline routed every other language's bare
/// filename into the symbol profile, which costs the path-shaped plan.
fn has_supported_file_extension(token: &str) -> bool {
    let Some((stem, extension)) = token.rsplit_once('.') else {
        return false;
    };
    if stem.is_empty() || extension.is_empty() {
        return false;
    }
    codestory_contracts::language_support::language_support_profile_for_ext(extension).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_symbol_like_queries() {
        let features = classify_query("ExtensionService");
        assert_eq!(features.shape, QueryShape::SymbolLike);
        let features = classify_query("foo::Bar");
        assert!(features.looks_like_qualified_symbol);
    }

    #[test]
    fn classifies_path_like_queries() {
        let features = classify_query("src/agent/orchestrator.rs");
        assert_eq!(features.shape, QueryShape::PathLike);
    }

    #[test]
    fn slash_separated_concepts_do_not_suppress_natural_language_intent() {
        for query in [
            "input/output",
            "src/runtime",
            "how does input/output validation work",
            "explain producer/consumer ownership",
        ] {
            let features = classify_query(query);
            assert!(!features.intent.path, "{query} is not a path lookup");
            assert!(features.intent.natural_language);
            assert_ne!(features.shape, QueryShape::PathLike);
        }
    }

    #[test]
    fn path_mentions_and_symbol_mentions_are_retained_as_multiple_intents() {
        let features = classify_query("explain how SearchWorker in src/worker.rs drains calls");
        assert_eq!(features.shape, QueryShape::Mixed);
        assert!(features.intent.symbol);
        assert!(features.intent.path);
        assert!(features.intent.natural_language);
        assert!(features.intent.relationship);
        assert!(!features.intent.standalone_path);
        assert_eq!(features.intent.exact_symbols, ["SearchWorker"]);
        assert_eq!(features.intent.paths, ["src/worker.rs"]);
        assert!(features.intent.relations.contains(&"calls".to_string()));
        assert!(
            features
                .intent
                .evidence_roles
                .contains(&"worker".to_string())
        );
        assert_eq!(features.intent.lookup_mode, QueryLookupMode::Relation);
    }

    #[test]
    fn ordered_flow_intent_keeps_roles_and_stages_independent() {
        let features = classify_query(
            "Explain how results flow through the search driver first and then reach a worker",
        );

        assert_eq!(features.intent.lookup_mode, QueryLookupMode::OrderedFlow);
        assert!(
            features
                .intent
                .evidence_roles
                .contains(&"driver".to_string())
        );
        assert!(
            features
                .intent
                .evidence_roles
                .contains(&"worker".to_string())
        );
        assert!(
            features
                .intent
                .ordered_flow_stages
                .contains(&"first".to_string())
        );
        assert!(
            features
                .intent
                .ordered_flow_stages
                .contains(&"then".to_string())
        );
    }

    #[test]
    fn classifies_natural_language() {
        let features = classify_query("how does packet retrieval work");
        assert_eq!(features.shape, QueryShape::NaturalLanguage);
    }

    #[test]
    fn sentence_capitalisation_and_punctuation_stay_natural_language() {
        for query in [
            "How does packet retrieval work",
            "Explain how the worker pool drains requests.",
            "Where is the request deadline enforced?",
        ] {
            let features = classify_query(query);
            assert!(
                !features.has_camel_case_token,
                "{query} has no interior camel hump"
            );
            assert!(
                !features.looks_like_qualified_symbol,
                "{query} has no interior qualifier"
            );
            assert_eq!(
                features.shape,
                QueryShape::NaturalLanguage,
                "{query} must keep the natural-language plan"
            );
        }
    }

    #[test]
    fn interior_camel_hump_and_qualifier_still_read_as_symbols() {
        let features = classify_query("SearchWorker");
        assert!(features.has_camel_case_token);
        assert_eq!(features.shape, QueryShape::SymbolLike);

        let features = classify_query("worker.spawn");
        assert!(features.looks_like_qualified_symbol);
        assert_eq!(features.shape, QueryShape::SymbolLike);

        let features = classify_query("Explain how SearchWorker drains requests");
        assert!(features.has_camel_case_token);
        assert_eq!(features.shape, QueryShape::Mixed);
    }

    #[test]
    fn bare_filenames_are_path_like_for_every_registered_extension() {
        for query in ["worker.py", "Worker.java", "worker.go", "worker.rs"] {
            assert_eq!(
                classify_query(query).shape,
                QueryShape::PathLike,
                "{query} is a bare filename of a supported language"
            );
        }
        assert_eq!(
            classify_query("worker.spawn").shape,
            QueryShape::SymbolLike,
            "an unregistered suffix stays a qualified symbol"
        );
    }
}
