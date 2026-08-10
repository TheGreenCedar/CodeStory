use serde::{Deserialize, Serialize};

/// High-level query shape used by the planner (repo-agnostic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryShape {
    SymbolLike,
    PathLike,
    NaturalLanguage,
    Mixed,
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryFeatures {
    pub raw_query: String,
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
    let intent = QueryIntent {
        symbol: symbol_like,
        path: path_like,
        natural_language: nl_like,
        relationship,
        standalone_symbol: symbol_like && !path_like && !nl_like && token_count == 1,
        standalone_path: path_like && !nl_like && token_count == 1,
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
        shape,
        intent,
        token_count,
        has_path_separators,
        has_camel_case_token,
        has_snake_case_token,
        looks_like_qualified_symbol,
    }
}

fn normalized_word(word: &str) -> String {
    word.trim_matches(|character: char| !character.is_alphanumeric() && character != '_')
        .to_ascii_lowercase()
}

fn looks_like_path_token(token: &str, standalone: bool) -> bool {
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
    let conventional_root = matches!(
        normalized.split('/').next().unwrap_or_default(),
        "app" | "apps" | "bin" | "crates" | "docs" | "lib" | "packages" | "src" | "test" | "tests"
    );
    conventional_root
        || (standalone
            && normalized
                .split('/')
                .filter(|component| !component.is_empty())
                .count()
                >= 3)
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
