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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryFeatures {
    pub raw_query: String,
    pub shape: QueryShape,
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
    let has_camel_case_token = tokens.iter().any(|token| has_internal_camel_hump(token));
    let has_snake_case_token = tokens.iter().any(|token| {
        token.contains('_')
            && token
                .chars()
                .filter(|c| c.is_ascii_alphabetic())
                .all(|c| c.is_ascii_lowercase() || c == '_')
    });
    let looks_like_qualified_symbol = tokens.iter().any(|token| is_qualified_symbol(token));

    let path_like =
        has_path_separators || (token_count == 1 && has_supported_file_extension(trimmed));
    let symbol_like = looks_like_qualified_symbol
        || has_camel_case_token
        || has_snake_case_token
        || (token_count == 1 && trimmed.chars().all(|c| c.is_alphanumeric() || c == '_'));

    let nl_like = token_count >= 3
        || trimmed.split_whitespace().any(|word| {
            matches!(
                word.to_ascii_lowercase().as_str(),
                "how" | "what" | "where" | "why" | "when" | "explain" | "find"
            )
        });

    let shape = if path_like {
        if symbol_like && nl_like {
            QueryShape::Mixed
        } else {
            QueryShape::PathLike
        }
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
        token_count,
        has_path_separators,
        has_camel_case_token,
        has_snake_case_token,
        looks_like_qualified_symbol,
    }
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
