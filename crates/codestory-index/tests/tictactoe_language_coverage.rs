use anyhow::{Result, anyhow};
use codestory_core::{Edge, EdgeKind, Node, NodeKind};
use codestory_index::{get_language_for_ext, index_file};
use std::path::Path;

const PYTHON_SOURCE: &str = include_str!("fixtures/tictactoe/python_tictactoe.py");
const JAVA_SOURCE: &str = include_str!("fixtures/tictactoe/java_tictactoe.java");
const RUST_SOURCE: &str = include_str!("fixtures/tictactoe/rust_tictactoe.rs");
const JAVASCRIPT_SOURCE: &str = include_str!("fixtures/tictactoe/javascript_tictactoe.js");
const TYPESCRIPT_SOURCE: &str = include_str!("fixtures/tictactoe/typescript_tictactoe.ts");
const CPP_SOURCE: &str = include_str!("fixtures/tictactoe/cpp_tictactoe.cpp");
const C_SOURCE: &str = include_str!("fixtures/tictactoe/c_tictactoe.c");

type NamePair = (&'static str, &'static str);

const PYTHON_SYMBOLS: &[(NodeKind, &str)] = &[
    (NodeKind::CLASS, "GameObject"),
    (NodeKind::CLASS, "Field"),
    (NodeKind::CLASS, "Player"),
    (NodeKind::CLASS, "HumanPlayer"),
    (NodeKind::CLASS, "ArtificialPlayer"),
    (NodeKind::CLASS, "TicTacToe"),
    (NodeKind::CLASS, "Move"),
    (NodeKind::CLASS, "Node"),
    (NodeKind::FUNCTION, "numberIn"),
    (NodeKind::FUNCTION, "numberOut"),
    (NodeKind::FUNCTION, "stringOut"),
    (NodeKind::FUNCTION, "sameInRow"),
    (NodeKind::FUNCTION, "makeMove"),
    (NodeKind::FUNCTION, "turn"),
    (NodeKind::FUNCTION, "_minMax"),
    (NodeKind::FUNCTION, "_input"),
    (NodeKind::FUNCTION, "_check"),
    (NodeKind::FUNCTION, "main"),
];
const JAVA_SYMBOLS: &[(NodeKind, &str)] = &[
    (NodeKind::CLASS, "Entry"),
    (NodeKind::CLASS, "GameObject"),
    (NodeKind::CLASS, "Field"),
    (NodeKind::CLASS, "Player"),
    (NodeKind::CLASS, "HumanPlayer"),
    (NodeKind::CLASS, "ArtificialPlayer"),
    (NodeKind::CLASS, "Move"),
    (NodeKind::CLASS, "Node"),
    (NodeKind::CLASS, "TicTacToe"),
    (NodeKind::FUNCTION, "numberIn"),
    (NodeKind::FUNCTION, "numberOut"),
    (NodeKind::FUNCTION, "stringOut"),
    (NodeKind::FUNCTION, "sameInRow"),
    (NodeKind::FUNCTION, "makeMove"),
    (NodeKind::FUNCTION, "turn"),
    (NodeKind::FUNCTION, "_input"),
    (NodeKind::FUNCTION, "_check"),
    (NodeKind::FUNCTION, "checkWinner"),
    (NodeKind::FUNCTION, "isDraw"),
    (NodeKind::FUNCTION, "probeCalls"),
    (NodeKind::FUNCTION, "run"),
    (NodeKind::FUNCTION, "main"),
    (NodeKind::FUNCTION, "_minMax"),
];
const RUST_SYMBOLS: &[(NodeKind, &str)] = &[
    (NodeKind::CLASS, "GameObject"),
    (NodeKind::CLASS, "Token"),
    (NodeKind::CLASS, "Move"),
    (NodeKind::CLASS, "Field"),
    (NodeKind::CLASS, "Player"),
    (NodeKind::CLASS, "HumanPlayer"),
    (NodeKind::CLASS, "ArtificialPlayer"),
    (NodeKind::CLASS, "Node"),
    (NodeKind::CLASS, "TicTacToe"),
    (NodeKind::FUNCTION, "number_in"),
    (NodeKind::FUNCTION, "number_out"),
    (NodeKind::FUNCTION, "string_out"),
    (NodeKind::FUNCTION, "same_in_row"),
    (NodeKind::FUNCTION, "make_move"),
    (NodeKind::FUNCTION, "check_winner"),
    (NodeKind::FUNCTION, "is_draw"),
    (NodeKind::FUNCTION, "turn"),
    (NodeKind::FUNCTION, "run"),
    (NodeKind::FUNCTION, "start"),
    (NodeKind::FUNCTION, "_select_player"),
    (NodeKind::FUNCTION, "main"),
    (NodeKind::FUNCTION, "min_max"),
];
const JAVASCRIPT_SYMBOLS: &[(NodeKind, &str)] = &[
    (NodeKind::CLASS, "GameObject"),
    (NodeKind::CLASS, "Field"),
    (NodeKind::CLASS, "Player"),
    (NodeKind::CLASS, "HumanPlayer"),
    (NodeKind::CLASS, "ArtificialPlayer"),
    (NodeKind::CLASS, "TicTacToe"),
    (NodeKind::FUNCTION, "numberIn"),
    (NodeKind::FUNCTION, "numberOut"),
    (NodeKind::FUNCTION, "stringOut"),
    (NodeKind::FUNCTION, "sameInRow"),
    (NodeKind::FUNCTION, "makeMove"),
    (NodeKind::FUNCTION, "turn"),
    (NodeKind::FUNCTION, "selectPlayer"),
    (NodeKind::FUNCTION, "start"),
    (NodeKind::FUNCTION, "run"),
    (NodeKind::FUNCTION, "probeCalls"),
    (NodeKind::FUNCTION, "main"),
    (NodeKind::FUNCTION, "checkWinner"),
    (NodeKind::FUNCTION, "isDraw"),
    (NodeKind::FUNCTION, "minMax"),
];
const TYPESCRIPT_SYMBOLS: &[(NodeKind, &str)] = &[
    (NodeKind::CLASS, "GameObject"),
    (NodeKind::CLASS, "Field"),
    (NodeKind::CLASS, "Player"),
    (NodeKind::CLASS, "HumanPlayer"),
    (NodeKind::CLASS, "ArtificialPlayer"),
    (NodeKind::CLASS, "TicTacToe"),
    (NodeKind::FUNCTION, "numberIn"),
    (NodeKind::FUNCTION, "numberOut"),
    (NodeKind::FUNCTION, "stringOut"),
    (NodeKind::FUNCTION, "sameInRow"),
    (NodeKind::FUNCTION, "makeMove"),
    (NodeKind::FUNCTION, "turn"),
    (NodeKind::FUNCTION, "selectPlayer"),
    (NodeKind::FUNCTION, "start"),
    (NodeKind::FUNCTION, "run"),
    (NodeKind::FUNCTION, "probeCalls"),
    (NodeKind::FUNCTION, "main"),
    (NodeKind::FUNCTION, "checkWinner"),
    (NodeKind::FUNCTION, "isDraw"),
    (NodeKind::FUNCTION, "minMax"),
];
const CPP_SYMBOLS: &[(NodeKind, &str)] = &[
    (NodeKind::CLASS, "GameObject"),
    (NodeKind::CLASS, "Field"),
    (NodeKind::CLASS, "Player"),
    (NodeKind::CLASS, "HumanPlayer"),
    (NodeKind::CLASS, "ArtificialPlayer"),
    (NodeKind::CLASS, "TicTacToe"),
    (NodeKind::FUNCTION, "same_in_row"),
    (NodeKind::FUNCTION, "make_move"),
    (NodeKind::FUNCTION, "turn"),
    (NodeKind::FUNCTION, "evaluate"),
    (NodeKind::FUNCTION, "run"),
    (NodeKind::FUNCTION, "start"),
    (NodeKind::FUNCTION, "main"),
    (NodeKind::FUNCTION, "check_winner"),
    (NodeKind::FUNCTION, "is_draw"),
    (NodeKind::FUNCTION, "min_max"),
];
const C_SYMBOLS: &[(NodeKind, &str)] = &[
    (NodeKind::FUNCTION, "number_in"),
    (NodeKind::FUNCTION, "number_out"),
    (NodeKind::FUNCTION, "string_out"),
    (NodeKind::FUNCTION, "opponent"),
    (NodeKind::FUNCTION, "clear_field"),
    (NodeKind::FUNCTION, "in_range"),
    (NodeKind::FUNCTION, "is_empty"),
    (NodeKind::FUNCTION, "same_in_row"),
    (NodeKind::FUNCTION, "make_move"),
    (NodeKind::FUNCTION, "clear_move"),
    (NodeKind::FUNCTION, "read_move"),
    (NodeKind::FUNCTION, "check_winner"),
    (NodeKind::FUNCTION, "is_draw"),
    (NodeKind::FUNCTION, "check_move"),
    (NodeKind::FUNCTION, "probe_check_winner"),
    (NodeKind::FUNCTION, "probe_is_draw"),
    (NodeKind::FUNCTION, "run"),
    (NodeKind::FUNCTION, "main"),
];

const PYTHON_IMPORTS: &[&str] = &["enum", "random"];
const JAVA_IMPORTS: &[&str] = &["ThreadLocalRandom"];
const RUST_IMPORTS: &[&str] = &["std::fmt"];
const JAVASCRIPT_IMPORTS: &[&str] = &["\"./helper.js\"", "\"./random.js\""];
const TYPESCRIPT_IMPORTS: &[&str] = &["\"./helper\"", "\"./random\""];
const CPP_IMPORTS: &[&str] = &["<array>", "<iostream>"];
const C_IMPORTS: &[&str] = &["<stdio.h>", "<stdbool.h>"];

const PYTHON_CALLS: &[&str] = &["numberIn", "sameInRow", "_check", "_minMax"];
const JAVA_CALLS: &[&str] = &["checkWinner", "isDraw", "_minMax"];
const RUST_CALLS: &[&str] = &["check_winner", "is_draw", "min_max"];
const JAVASCRIPT_CALLS: &[&str] = &["checkWinner", "isDraw", "minMax"];
const TYPESCRIPT_CALLS: &[&str] = &["checkWinner", "isDraw", "minMax"];
const CPP_CALLS: &[&str] = &["check_winner", "is_draw", "min_max"];
const C_CALLS: &[&str] = &["check_winner", "is_draw", "check_move"];

const PYTHON_MEMBERS: &[NamePair] = &[
    ("Field", "makeMove"),
    ("HumanPlayer", "_input"),
    ("ArtificialPlayer", "_minMax"),
    ("TicTacToe", "run"),
];
const JAVA_MEMBERS: &[NamePair] = &[
    ("Field", "makeMove"),
    ("HumanPlayer", "_input"),
    ("ArtificialPlayer", "_minMax"),
    ("TicTacToe", "run"),
];
const RUST_MEMBERS: &[NamePair] = &[
    ("Field", "make_move"),
    ("HumanPlayer", "turn"),
    ("ArtificialPlayer", "min_max"),
    ("TicTacToe", "run"),
];
const JAVASCRIPT_MEMBERS: &[NamePair] = &[
    ("Field", "makeMove"),
    ("HumanPlayer", "turn"),
    ("ArtificialPlayer", "minMax"),
    ("TicTacToe", "run"),
];
const TYPESCRIPT_MEMBERS: &[NamePair] = &[
    ("Field", "makeMove"),
    ("HumanPlayer", "turn"),
    ("ArtificialPlayer", "minMax"),
    ("TicTacToe", "run"),
];
const CPP_MEMBERS: &[NamePair] = &[
    ("Field", "make_move"),
    ("HumanPlayer", "turn"),
    ("ArtificialPlayer", "min_max"),
    ("TicTacToe", "run"),
];
const C_MEMBERS: &[NamePair] = &[];

const PYTHON_INHERITANCE: &[NamePair] = &[
    ("Field", "GameObject"),
    ("HumanPlayer", "Player"),
    ("ArtificialPlayer", "Player"),
];
const JAVA_INHERITANCE: &[NamePair] = &[
    ("Field", "GameObject"),
    ("HumanPlayer", "Player"),
    ("ArtificialPlayer", "Player"),
];
const RUST_INHERITANCE: &[NamePair] = &[("HumanPlayer", "Player"), ("ArtificialPlayer", "Player")];
const JAVASCRIPT_INHERITANCE: &[NamePair] = &[
    ("Field", "GameObject"),
    ("HumanPlayer", "Player"),
    ("ArtificialPlayer", "Player"),
    ("TicTacToe", "GameObject"),
];
const TYPESCRIPT_INHERITANCE: &[NamePair] = &[
    ("Field", "GameObject"),
    ("HumanPlayer", "Player"),
    ("ArtificialPlayer", "Player"),
    ("TicTacToe", "GameObject"),
];
const CPP_INHERITANCE: &[NamePair] = &[("TicTacToe", "GameObject")];
const C_INHERITANCE: &[NamePair] = &[];

#[derive(Clone, Copy)]
struct FixtureCase {
    language: &'static str,
    filename: &'static str,
    extension: &'static str,
    source: &'static str,
    min_nodes: usize,
    min_edges: usize,
    required_symbols: &'static [(NodeKind, &'static str)],
    required_import_targets: &'static [&'static str],
    required_call_targets: &'static [&'static str],
    required_member_pairs: &'static [NamePair],
    required_inheritance_pairs: &'static [NamePair],
}

fn fixture_cases() -> Vec<FixtureCase> {
    vec![
        FixtureCase {
            language: "python",
            filename: "game.py",
            extension: "py",
            source: PYTHON_SOURCE,
            min_nodes: 25,
            min_edges: 20,
            required_symbols: PYTHON_SYMBOLS,
            required_import_targets: PYTHON_IMPORTS,
            required_call_targets: PYTHON_CALLS,
            required_member_pairs: PYTHON_MEMBERS,
            required_inheritance_pairs: PYTHON_INHERITANCE,
        },
        FixtureCase {
            language: "java",
            filename: "Game.java",
            extension: "java",
            source: JAVA_SOURCE,
            min_nodes: 20,
            min_edges: 20,
            required_symbols: JAVA_SYMBOLS,
            required_import_targets: JAVA_IMPORTS,
            required_call_targets: JAVA_CALLS,
            required_member_pairs: JAVA_MEMBERS,
            required_inheritance_pairs: JAVA_INHERITANCE,
        },
        FixtureCase {
            language: "rust",
            filename: "main.rs",
            extension: "rs",
            source: RUST_SOURCE,
            min_nodes: 20,
            min_edges: 16,
            required_symbols: RUST_SYMBOLS,
            required_import_targets: RUST_IMPORTS,
            required_call_targets: RUST_CALLS,
            required_member_pairs: RUST_MEMBERS,
            required_inheritance_pairs: RUST_INHERITANCE,
        },
        FixtureCase {
            language: "javascript",
            filename: "game.js",
            extension: "js",
            source: JAVASCRIPT_SOURCE,
            min_nodes: 20,
            min_edges: 18,
            required_symbols: JAVASCRIPT_SYMBOLS,
            required_import_targets: JAVASCRIPT_IMPORTS,
            required_call_targets: JAVASCRIPT_CALLS,
            required_member_pairs: JAVASCRIPT_MEMBERS,
            required_inheritance_pairs: JAVASCRIPT_INHERITANCE,
        },
        FixtureCase {
            language: "typescript",
            filename: "game.ts",
            extension: "ts",
            source: TYPESCRIPT_SOURCE,
            min_nodes: 20,
            min_edges: 18,
            required_symbols: TYPESCRIPT_SYMBOLS,
            required_import_targets: TYPESCRIPT_IMPORTS,
            required_call_targets: TYPESCRIPT_CALLS,
            required_member_pairs: TYPESCRIPT_MEMBERS,
            required_inheritance_pairs: TYPESCRIPT_INHERITANCE,
        },
        FixtureCase {
            language: "cpp",
            filename: "game.cpp",
            extension: "cpp",
            source: CPP_SOURCE,
            min_nodes: 16,
            min_edges: 14,
            required_symbols: CPP_SYMBOLS,
            required_import_targets: CPP_IMPORTS,
            required_call_targets: CPP_CALLS,
            required_member_pairs: CPP_MEMBERS,
            required_inheritance_pairs: CPP_INHERITANCE,
        },
        FixtureCase {
            language: "c",
            filename: "game.c",
            extension: "c",
            source: C_SOURCE,
            min_nodes: 12,
            min_edges: 10,
            required_symbols: C_SYMBOLS,
            required_import_targets: C_IMPORTS,
            required_call_targets: C_CALLS,
            required_member_pairs: C_MEMBERS,
            required_inheritance_pairs: C_INHERITANCE,
        },
    ]
}

fn index_case(case: &FixtureCase) -> Result<codestory_index::IndexResult> {
    let (language, language_name, graph_query) = get_language_for_ext(case.extension)
        .ok_or_else(|| anyhow!("No language mapping for extension '{}'", case.extension))?;

    index_file(
        Path::new(case.filename),
        case.source,
        language,
        language_name,
        graph_query,
        None,
        None,
    )
    .map_err(|e| anyhow!("Indexing failed for {}: {}", case.language, e))
}

fn is_matching_name(serialized_name: &str, wanted_name: &str) -> bool {
    serialized_name == wanted_name
        || serialized_name.ends_with(&format!(".{wanted_name}"))
        || serialized_name.ends_with(&format!("::{wanted_name}"))
        || serialized_name.ends_with(&format!(" {wanted_name}"))
}

fn has_node(nodes: &[Node], kind: NodeKind, name: &str) -> bool {
    nodes.iter().any(|node| {
        let kind_matches = if kind == NodeKind::FUNCTION {
            node.kind == NodeKind::FUNCTION || node.kind == NodeKind::METHOD
        } else if kind == NodeKind::VARIABLE {
            node.kind == NodeKind::VARIABLE || node.kind == NodeKind::FIELD
        } else {
            node.kind == kind
        };
        kind_matches && is_matching_name(&node.serialized_name, name)
    })
}

fn has_edge_kind(edges: &[Edge], kind: EdgeKind) -> bool {
    edges.iter().any(|edge| edge.kind == kind)
}

fn has_edge_target_name(edges: &[Edge], nodes: &[Node], kind: EdgeKind, target_name: &str) -> bool {
    edges.iter().filter(|edge| edge.kind == kind).any(|edge| {
        nodes
            .iter()
            .find(|node| node.id == edge.target)
            .map(|node| is_matching_name(&node.serialized_name, target_name))
            .unwrap_or(false)
    })
}

fn has_edge_between_names(
    edges: &[Edge],
    nodes: &[Node],
    kind: EdgeKind,
    source_name: &str,
    target_name: &str,
) -> bool {
    edges.iter().filter(|edge| edge.kind == kind).any(|edge| {
        let source_matches = nodes
            .iter()
            .find(|node| node.id == edge.source)
            .map(|node| is_matching_name(&node.serialized_name, source_name))
            .unwrap_or(false);

        let target_matches = nodes
            .iter()
            .find(|node| node.id == edge.target)
            .map(|node| is_matching_name(&node.serialized_name, target_name))
            .unwrap_or(false);

        source_matches && target_matches
    })
}

#[test]
fn test_language_extension_coverage_and_names() {
    let expected = [
        ("py", "python"),
        ("java", "java"),
        ("rs", "rust"),
        ("js", "javascript"),
        ("ts", "typescript"),
        ("tsx", "typescript"),
        ("cpp", "cpp"),
        ("cc", "cpp"),
        ("cxx", "cpp"),
        ("h", "cpp"),
        ("hpp", "cpp"),
        ("c", "c"),
    ];

    for (ext, expected_name) in expected {
        let (_, language_name, graph_query) =
            get_language_for_ext(ext).expect("Extension should resolve to a language");
        assert_eq!(
            language_name, expected_name,
            "Wrong language name for extension {ext}"
        );
        assert!(
            !graph_query.trim().is_empty(),
            "Expected non-empty graph query for extension {ext}"
        );
    }
}

#[test]
fn test_tictactoe_fixture_parses_for_all_supported_languages() -> Result<()> {
    for case in fixture_cases() {
        let result = index_case(&case)?;
        assert!(
            result.nodes.len() >= case.min_nodes,
            "Expected at least {} nodes for {}, got {}",
            case.min_nodes,
            case.language,
            result.nodes.len()
        );
        assert!(
            result.edges.len() >= case.min_edges,
            "Expected at least {} edges for {}, got {}",
            case.min_edges,
            case.language,
            result.edges.len()
        );
    }
    Ok(())
}

#[test]
fn test_tictactoe_core_symbols_present_per_language() -> Result<()> {
    for case in fixture_cases() {
        let result = index_case(&case)?;
        for (kind, name) in case.required_symbols {
            assert!(
                has_node(&result.nodes, *kind, name),
                "Missing {kind:?} node '{name}' for {}",
                case.language
            );
        }
    }
    Ok(())
}

#[test]
fn test_tictactoe_edges_cover_import_member_and_call() -> Result<()> {
    for case in fixture_cases() {
        let result = index_case(&case)?;

        assert!(
            has_edge_kind(&result.edges, EdgeKind::IMPORT),
            "Missing IMPORT edges for {}",
            case.language
        );
        assert!(
            has_edge_kind(&result.edges, EdgeKind::CALL),
            "Missing CALL edges for {}",
            case.language
        );

        if !case.required_member_pairs.is_empty() {
            assert!(
                has_edge_kind(&result.edges, EdgeKind::MEMBER),
                "Missing MEMBER edges for {}",
                case.language
            );
        }

        if !case.required_inheritance_pairs.is_empty() {
            assert!(
                has_edge_kind(&result.edges, EdgeKind::INHERITANCE),
                "Missing INHERITANCE edge for {}",
                case.language
            );
        }
    }
    Ok(())
}

#[test]
fn test_tictactoe_import_targets_are_extracted() -> Result<()> {
    for case in fixture_cases() {
        let result = index_case(&case)?;
        for target in case.required_import_targets {
            assert!(
                has_edge_target_name(&result.edges, &result.nodes, EdgeKind::IMPORT, target),
                "Missing IMPORT edge target '{target}' for {}",
                case.language
            );
        }
    }
    Ok(())
}

#[test]
fn test_tictactoe_nested_calls_are_extracted() -> Result<()> {
    for case in fixture_cases() {
        let result = index_case(&case)?;
        for target in case.required_call_targets {
            assert!(
                has_edge_target_name(&result.edges, &result.nodes, EdgeKind::CALL, target),
                "Missing CALL edge to '{target}' for {}",
                case.language
            );
        }
    }
    Ok(())
}

#[test]
fn test_tictactoe_member_relationships_are_extracted() -> Result<()> {
    for case in fixture_cases() {
        if case.required_member_pairs.is_empty() {
            continue;
        }

        let result = index_case(&case)?;
        for (owner, member) in case.required_member_pairs {
            assert!(
                has_edge_between_names(
                    &result.edges,
                    &result.nodes,
                    EdgeKind::MEMBER,
                    owner,
                    member
                ),
                "Missing MEMBER edge '{} -> {}' for {}",
                owner,
                member,
                case.language
            );
        }
    }
    Ok(())
}

#[test]
fn test_tictactoe_inheritance_relationships_are_extracted() -> Result<()> {
    for case in fixture_cases() {
        if case.required_inheritance_pairs.is_empty() {
            continue;
        }

        let result = index_case(&case)?;
        for (child, parent) in case.required_inheritance_pairs {
            assert!(
                has_edge_between_names(
                    &result.edges,
                    &result.nodes,
                    EdgeKind::INHERITANCE,
                    child,
                    parent
                ),
                "Missing INHERITANCE edge '{} -> {}' for {}",
                child,
                parent,
                case.language
            );
        }
    }
    Ok(())
}
