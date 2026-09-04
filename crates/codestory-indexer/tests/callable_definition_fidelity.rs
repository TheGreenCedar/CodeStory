use codestory_contracts::graph::{EdgeKind, NodeKind};
use codestory_indexer::{get_language_for_ext, index_file};
use std::path::Path;

const NATIVE_FORMS: &[(&str, &str)] = &[
    ("scalar", "int TOKEN(int x) {\n return x + 1;\n}\n"),
    ("pointer", "int *TOKEN(int *x) {\n return x;\n}\n"),
    ("nested_pointer", "int **TOKEN(int **x) {\n return x;\n}\n"),
    (
        "qualified_pointer",
        "static const int *TOKEN(const int *x) {\n return x;\n}\n",
    ),
    ("parenthesized", "int (TOKEN)(int x) {\n return x;\n}\n"),
    (
        "returned_callable",
        "int (*TOKEN(void))(int) {\n return 0;\n}\n",
    ),
    (
        "calling_convention",
        "int (__stdcall TOKEN)(int x) {\n return x;\n}\n",
    ),
    (
        "returned_calling_convention",
        "int (__cdecl *TOKEN(void))(int) {\n return 0;\n}\n",
    ),
];

const SCRIPT_FORMS: &[(&str, &str)] = &[
    (
        "declaration",
        "function TOKEN(x) {\n return x + 1;\n}\nTOKEN(1);\n",
    ),
    (
        "arrow",
        "const TOKEN = (x) => {\n return x + 1;\n};\nTOKEN(1);\n",
    ),
    (
        "named_expression",
        "var TOKEN = function privateName(x) {\n return x + 1;\n};\nTOKEN(1);\n",
    ),
    (
        "anonymous_expression",
        "const TOKEN = function(x) {\n return x + 1;\n};\nTOKEN(1);\n",
    ),
    (
        "async_expression",
        "let TOKEN = async function(x) {\n return x + 1;\n};\nTOKEN(1);\n",
    ),
    (
        "generator_expression",
        "const TOKEN = function*(x) {\n yield x + 1;\n};\nTOKEN(1);\n",
    ),
    (
        "generator_declaration",
        "function* TOKEN(x) {\n yield x + 1;\n}\nTOKEN(1);\n",
    ),
    (
        "async_generator_expression",
        "const TOKEN = async function*(x) {\n yield x + 1;\n};\nTOKEN(1);\n",
    ),
];

fn assert_definition_matrix(extensions: &[&str], forms: &[(&str, &str)]) {
    let mut failures = Vec::new();
    for extension in extensions {
        let config = get_language_for_ext(extension).expect("supported parser");
        for (form, template) in forms {
            for (name, prefix, directory) in [
                ("pebble", "", "alpha"),
                ("opaque_942", "// shifted source\n\n", "unrelated"),
            ] {
                let source = format!("{prefix}{}", template.replace("TOKEN", name));
                let filename = format!("{directory}/fixture.{extension}");
                let result = index_file(Path::new(&filename), &source, &config, None, None)
                    .expect("index valid source");
                assert!(
                    result.files.iter().all(|file| file.complete),
                    "{filename} {form}"
                );
                let start = prefix.lines().count() as u32 + 1;
                let definitions = result
                    .nodes
                    .iter()
                    .filter(|node| {
                        node.kind == NodeKind::FUNCTION
                            && node.serialized_name == name
                            && node.start_line == Some(start)
                            && node.end_line == Some(start + 2)
                    })
                    .count();
                if definitions != 1 {
                    failures.push(format!("{extension}/{form}/{name}: expected one complete definition, got {definitions}; nodes={:?}", result.nodes));
                }
                assert!(
                    !result.nodes.iter().any(|node| {
                        node.kind == NodeKind::FUNCTION && node.serialized_name == "privateName"
                    }),
                    "a named expression's private name must not become an unscoped alias"
                );
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} definition failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn native_definitions_follow_nested_declarators() {
    assert_definition_matrix(&["c", "cpp"], NATIVE_FORMS);
}

#[test]
fn script_definitions_follow_callable_bindings() {
    assert_definition_matrix(&["js", "ts", "tsx"], SCRIPT_FORMS);
}

#[test]
fn non_callable_names_do_not_become_definitions() {
    for extension in ["c", "cpp", "js", "ts", "tsx"] {
        let source = if matches!(extension, "c" | "cpp") {
            "int value;\nint (*callback)(int parameter);\nint owner(int parameter) {\n if (parameter) { return callback(parameter); }\n return value;\n}\n"
        } else {
            "const value = 1;\nfunction owner(parameter) {\n if (parameter) { return callback(parameter); }\n return value;\n}\n"
        };
        let config = get_language_for_ext(extension).expect("supported parser");
        let filename = format!("negative.{extension}");
        let result = index_file(Path::new(&filename), source, &config, None, None).expect("index");
        let names = result
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::FUNCTION)
            .map(|node| node.serialized_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            ["owner"],
            "{extension} must not promote parameters, variables, or calls"
        );
    }
}

#[test]
fn native_definition_names_do_not_depend_on_pointer_depth() {
    for extension in ["c", "cpp"] {
        let config = get_language_for_ext(extension).expect("native parser");
        for depth in [1, 2, 8, 32] {
            let pointers = "*".repeat(depth);
            let source =
                format!("int {pointers}pebble(int {pointers}argument) {{\n return argument;\n}}\n");
            let filename = format!("depth.{extension}");
            let result =
                index_file(Path::new(&filename), &source, &config, None, None).expect("index");
            let names = result
                .nodes
                .iter()
                .filter(|node| node.kind == NodeKind::FUNCTION)
                .map(|node| node.serialized_name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(names, ["pebble"], "{extension} pointer depth {depth}");
        }
    }
}

#[test]
fn native_non_definition_callables_keep_their_existing_projection() {
    for (extension, source, name) in [
        ("c", "int pebble(int argument);\n", "pebble"),
        (
            "cpp",
            "auto pebble = [](int argument) { return argument; };\n",
            "pebble",
        ),
    ] {
        let config = get_language_for_ext(extension).expect("native parser");
        let filename = format!("existing.{extension}");
        let result = index_file(Path::new(&filename), source, &config, None, None).expect("index");
        assert!(
            result
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::FUNCTION && node.serialized_name == name),
            "{extension}: {:?}",
            result.nodes
        );
    }
}

#[test]
fn cpp_declarators_keep_reference_and_qualified_names() {
    let forms = [
        ("int &pebble(int &x) { return x; }", "pebble"),
        (
            "int &&pebble(int &&x) { return static_cast<int&&>(x); }",
            "pebble",
        ),
        (
            "struct Stone { int &pebble(int &x) { return x; } };",
            "Stone::pebble",
        ),
        ("struct Stone { ~Stone() {} };", "Stone::~Stone"),
        (
            "struct Stone { int operator()(int x) { return x; } };",
            "Stone::operator()",
        ),
    ];
    let config = get_language_for_ext("cpp").expect("C++ parser");
    let mut failures = Vec::new();
    for (source, name) in forms {
        let result =
            index_file(Path::new("forms.cpp"), source, &config, None, None).expect("index");
        assert!(result.files.iter().all(|file| file.complete));
        if !result
            .nodes
            .iter()
            .any(|node| node.kind == NodeKind::FUNCTION && node.serialized_name == name)
        {
            failures.push(format!("{source}: missing {name}; {:?}", result.nodes));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn cpp_specializations_keep_their_own_definition_identity() {
    for (name, prefix, directory) in [
        ("pebble", "", "alpha"),
        ("opaque_942", "// shifted\n\n", "elsewhere"),
    ] {
        for (open, close) in [("", ""), ("namespace opaque_space {\n", "}\n")] {
            for pointer in ["", "*"] {
                let source = format!(
                    "{prefix}{open}template<class T> T {pointer}{name}(T {pointer}x) {{ return x; }}\ntemplate<> int {pointer}{name}<int>(int {pointer}x) {{\n return x;\n}}\n{close}"
                );
                let config = get_language_for_ext("cpp").expect("C++ parser");
                let result = index_file(
                    Path::new(&format!("{directory}/special.cpp")),
                    &source,
                    &config,
                    None,
                    None,
                )
                .expect("index");
                assert!(result.files.iter().all(|file| file.complete));
                let specialization = format!("{name}<int>");
                let target = result
                    .nodes
                    .iter()
                    .find(|node| {
                        node.kind == NodeKind::FUNCTION
                            && node.serialized_name == specialization
                    })
                    .unwrap_or_else(|| {
                        panic!("missing {specialization} for {source}: {:?}", result.nodes)
                    });
                let start = prefix.lines().count() as u32 + open.lines().count() as u32 + 2;
                assert_eq!(
                    (target.start_line, target.end_line),
                    (Some(start), Some(start + 2))
                );
            }
        }
    }
}

#[test]
fn native_member_edges_keep_the_definition_identity() {
    let source = "namespace sample {\nstruct Holder {\n int **pick(int **argument) {\n  return argument;\n }\n};\nint **loose(int **argument) {\n return argument;\n}\n}\n";
    let config = get_language_for_ext("cpp").expect("C++ parser");
    let result = index_file(Path::new("members.cpp"), source, &config, None, None).expect("index");
    for (owner, member, start, end) in [("Holder", "Holder::pick", 3, 5), ("sample", "loose", 7, 9)]
    {
        let owner = result
            .nodes
            .iter()
            .find(|node| node.serialized_name == owner)
            .expect("owner");
        let target = result
            .nodes
            .iter()
            .find(|node| node.serialized_name == member && node.kind == NodeKind::FUNCTION)
            .unwrap_or_else(|| panic!("missing {member} definition: {:?}", result.nodes));
        assert_eq!(
            (target.start_line, target.end_line),
            (Some(start), Some(end))
        );
        assert!(result.edges.iter().any(|edge| edge.kind == EdgeKind::MEMBER
            && edge.source == owner.id
            && edge.target == target.id));
    }
}
