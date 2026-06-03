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
    let has_camel_case_token = tokens.iter().any(|token| {
        token.chars().any(char::is_uppercase) && token.chars().any(char::is_lowercase)
    });
    let has_snake_case_token = tokens.iter().any(|token| {
        token.contains('_')
            && token
                .chars()
                .filter(|c| c.is_ascii_alphabetic())
                .all(|c| c.is_ascii_lowercase() || c == '_')
    });
    let looks_like_qualified_symbol = tokens
        .iter()
        .any(|token| token.contains('.') || token.contains("::"));

    let path_like = has_path_separators
        || (token_count == 1
            && (trimmed.ends_with(".rs") || trimmed.ends_with(".ts") || trimmed.ends_with(".tsx")));
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
}
