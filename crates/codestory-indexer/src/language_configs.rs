use super::{
    BASH_GRAPH_QUERY, C_GRAPH_QUERY, CPP_GRAPH_QUERY, CSHARP_GRAPH_QUERY, DART_GRAPH_QUERY,
    GO_GRAPH_QUERY, JAVA_GRAPH_QUERY, JAVASCRIPT_GRAPH_QUERY, KOTLIN_GRAPH_QUERY, LanguageConfig,
    LanguageRuleset, PHP_GRAPH_QUERY, PYTHON_GRAPH_QUERY, RUBY_GRAPH_QUERY, RUST_GRAPH_QUERY,
    RUST_TAGS_QUERY, SWIFT_GRAPH_QUERY, TSX_GRAPH_QUERY, TSX_TAGS_QUERY, TYPESCRIPT_GRAPH_QUERY,
    TYPESCRIPT_TAGS_QUERY, make_language_config,
};
use codestory_contracts::language_support::{
    LanguageSupportMode, language_support_profile_for_ext, normalize_extension,
};

pub(super) fn get_language_for_ext(ext: &str) -> Option<LanguageConfig> {
    let ext = normalize_extension(ext);
    let profile = language_support_profile_for_ext(&ext)?;
    if profile.support_mode != LanguageSupportMode::ParserBackedGraph {
        return None;
    }

    match (profile.language_name, ext.as_str()) {
        ("python", _) => Some(python()),
        ("java", _) => Some(java()),
        ("rust", _) => Some(rust()),
        ("javascript", _) => Some(javascript()),
        ("typescript", "tsx") => Some(tsx()),
        ("typescript", _) => Some(typescript()),
        ("cpp", _) => Some(cpp()),
        ("c", _) => Some(c()),
        ("go", _) => Some(go()),
        ("ruby", _) => Some(ruby()),
        ("php", _) => Some(php()),
        ("csharp", _) => Some(csharp()),
        ("kotlin", _) => Some(kotlin()),
        ("swift", _) => Some(swift()),
        ("dart", _) => Some(dart()),
        ("bash", _) => Some(bash()),
        _ => None,
    }
}

fn python() -> LanguageConfig {
    make_language_config(
        tree_sitter_python::LANGUAGE.into(),
        "python",
        PYTHON_GRAPH_QUERY,
        None,
        LanguageRuleset::Python,
    )
}

fn java() -> LanguageConfig {
    make_language_config(
        tree_sitter_java::LANGUAGE.into(),
        "java",
        JAVA_GRAPH_QUERY,
        None,
        LanguageRuleset::Java,
    )
}

fn rust() -> LanguageConfig {
    make_language_config(
        tree_sitter_rust::LANGUAGE.into(),
        "rust",
        RUST_GRAPH_QUERY,
        Some(RUST_TAGS_QUERY),
        LanguageRuleset::Rust,
    )
}

fn javascript() -> LanguageConfig {
    make_language_config(
        tree_sitter_javascript::LANGUAGE.into(),
        "javascript",
        JAVASCRIPT_GRAPH_QUERY,
        None,
        LanguageRuleset::JavaScript,
    )
}

fn typescript() -> LanguageConfig {
    make_language_config(
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "typescript",
        TYPESCRIPT_GRAPH_QUERY,
        Some(TYPESCRIPT_TAGS_QUERY),
        LanguageRuleset::TypeScript,
    )
}

fn tsx() -> LanguageConfig {
    make_language_config(
        tree_sitter_typescript::LANGUAGE_TSX.into(),
        "typescript",
        TSX_GRAPH_QUERY,
        Some(TSX_TAGS_QUERY),
        LanguageRuleset::Tsx,
    )
}

fn cpp() -> LanguageConfig {
    make_language_config(
        tree_sitter_cpp::LANGUAGE.into(),
        "cpp",
        CPP_GRAPH_QUERY,
        None,
        LanguageRuleset::Cpp,
    )
}

fn c() -> LanguageConfig {
    make_language_config(
        tree_sitter_c::LANGUAGE.into(),
        "c",
        C_GRAPH_QUERY,
        None,
        LanguageRuleset::C,
    )
}

fn go() -> LanguageConfig {
    make_language_config(
        tree_sitter_go::LANGUAGE.into(),
        "go",
        GO_GRAPH_QUERY,
        None,
        LanguageRuleset::Go,
    )
}

fn ruby() -> LanguageConfig {
    make_language_config(
        tree_sitter_ruby::LANGUAGE.into(),
        "ruby",
        RUBY_GRAPH_QUERY,
        None,
        LanguageRuleset::Ruby,
    )
}

fn php() -> LanguageConfig {
    make_language_config(
        tree_sitter_php::LANGUAGE_PHP.into(),
        "php",
        PHP_GRAPH_QUERY,
        None,
        LanguageRuleset::Php,
    )
}

fn csharp() -> LanguageConfig {
    make_language_config(
        tree_sitter_c_sharp::LANGUAGE.into(),
        "csharp",
        CSHARP_GRAPH_QUERY,
        None,
        LanguageRuleset::CSharp,
    )
}

fn kotlin() -> LanguageConfig {
    make_language_config(
        tree_sitter_kotlin_ng::LANGUAGE.into(),
        "kotlin",
        KOTLIN_GRAPH_QUERY,
        None,
        LanguageRuleset::Kotlin,
    )
}

fn swift() -> LanguageConfig {
    make_language_config(
        tree_sitter_swift::LANGUAGE.into(),
        "swift",
        SWIFT_GRAPH_QUERY,
        None,
        LanguageRuleset::Swift,
    )
}

fn dart() -> LanguageConfig {
    make_language_config(
        tree_sitter_dart_orchard::LANGUAGE.into(),
        "dart",
        DART_GRAPH_QUERY,
        None,
        LanguageRuleset::Dart,
    )
}

fn bash() -> LanguageConfig {
    make_language_config(
        tree_sitter_bash::LANGUAGE.into(),
        "bash",
        BASH_GRAPH_QUERY,
        None,
        LanguageRuleset::Bash,
    )
}
