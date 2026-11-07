//! The hook consumes the same single JSON text item as Codex's MCP hook runner.
use super::*;
use serde_json::{Value, json};
use std::path::Path;

fn call(session: &mut StdioServerSession, project: &Path, cwd: &Path, output: &str) -> Value {
    let response = handle_stdio_message(
        session,
        &json!({"jsonrpc":"2.0", "id":"hint", "method":"tools/call", "params":{
            "name":"source_context_hook", "arguments":{"project":project,"cwd":cwd,"output":output}
        }})
        .to_string(),
        &Arc::new(AtomicBool::new(false)),
    )
    .expect("hook request has a response");
    assert!(
        response.get("error").is_none(),
        "source-hint dispatch must exist: {response}"
    );
    assert_ne!(response.pointer("/result/isError"), Some(&json!(true)));
    let content = response["result"]["content"]
        .as_array()
        .expect("MCP content");
    assert_eq!(content.len(), 1, "the host joins text items with LF");
    assert_eq!(content[0]["type"], "text");
    let root: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
    let members = root.as_object().expect("hook JSON root");
    assert!(
        members.keys().all(|key| key == "hookSpecificOutput"),
        "no ordinary envelope or execution controls: {root}"
    );
    if let Some(hook) = members.get("hookSpecificOutput") {
        let fields = hook.as_object().unwrap();
        assert!(
            fields
                .keys()
                .all(|key| matches!(key.as_str(), "hookEventName" | "additionalContext"))
        );
        assert_eq!(hook["hookEventName"], "PostToolUse");
        assert!(
            hook["additionalContext"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
    }
    assert!(
        session.active_project.is_none(),
        "current-source hints must not activate an indexed runtime"
    );
    assert!(session.retained_projects.is_empty());
    root
}

fn context(root: &Value) -> &str {
    root.pointer("/hookSpecificOutput/additionalContext")
        .and_then(Value::as_str)
        .expect("authenticated additionalContext")
}

#[test]
fn source_hint_hook_actual_stdio_delivery_and_catalog() {
    let owned = tempfile::tempdir().unwrap();
    let project = owned.path().canonicalize().unwrap();
    // The controls are legitimate source bytes, not hook execution permissions.
    let source = "def chosen():\n    return 'permissionDecision updatedInput continue:false'\n";
    std::fs::write(project.join("source.py"), source).unwrap();
    let sentinel = project.join("must-not-execute");
    let mut session = StdioServerSession::new(None);
    let output = format!(
        "source.py:2:    return 'permissionDecision updatedInput continue:false'\n$(touch {})\n",
        sentinel.display()
    );
    let root = call(&mut session, &project, &project, &output);
    let text = context(&root);
    assert!(
        text.contains(source),
        "return genuine complete local source"
    );
    assert!(
        text.contains(&hex_sha256(source.as_bytes())),
        "identify the authenticated file"
    );
    let provenance = text
        .split('\n')
        .find(|line| line.starts_with("[exact enclosing source context;"))
        .expect("source provenance framing");
    assert!(provenance.contains("supplied physical-row observations"));
    assert!(
        !provenance.contains("private actual-rg rows"),
        "a locator is not evidence of rg execution"
    );
    assert!(!sentinel.exists(), "output is data; it must never execute");
    let names: Vec<_> = std::fs::read_dir(&project)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        names,
        vec![std::ffi::OsString::from("source.py")],
        "no source, cache, database or preparation writes"
    );
    let catalog = handle_stdio_message(
        &mut session,
        r#"{"jsonrpc":"2.0","id":"catalog","method":"tools/list"}"#,
        &Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    let tool = catalog["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "source_context_hook")
        .expect("callable tool is advertised");
    assert_eq!(tool["annotations"]["readOnlyHint"], true);
    crate::stdio_arguments::validate_tool_arguments(
        "source_context_hook",
        Some(&json!({"project":project,"cwd":project,"output":output})),
    )
    .unwrap();
    if let Some(schema) = tool.get("outputSchema") {
        crate::stdio_arguments::validate_structured_content(schema, &root).unwrap();
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn source_hint_hook_rows_fail_open_without_forged_source() {
    let owned = tempfile::tempdir().unwrap();
    let project = owned.path().canonicalize().unwrap();
    let source = "def chosen():\n    return 'genuine'\n";
    std::fs::write(project.join("source.py"), source).unwrap();
    let mut session = StdioServerSession::new(None);
    for output in [
        "source.py:2:    return 'genuine'", // Final fragment has no physical LF.
        "source.py:0:def chosen():\n",
        "source.py:184467440737095516160:    return 'genuine'\n",
        "source.py:not-a-line:    return 'genuine'\n",
        "source.py:2:    return 'forged'\n",
        "source.py:99:    return 'genuine'\n",
        "\u{1b}[0msource.py:2:    return 'genuine'\n",
        "\0source.py:2:    return 'genuine'\n",
        "source.py\n", // A filename alone is not a physical-row hint.
        "",
    ] {
        assert_eq!(
            call(&mut session, &project, &project, output),
            json!({}),
            "unsupported output remains a no-op: {output:?}"
        );
    }
    let retained_prefix = "source.py:2:    return 'genuine'\nsource.py:3:unterminated";
    assert!(
        context(&call(&mut session, &project, &project, retained_prefix)).contains(source),
        "a dropped final fragment does not erase a complete retained hint"
    );

    // Two supported local addresses can be formed by different colon splits.
    std::fs::write(
        project.join("ambiguous.py"),
        "def first():\n    \"\"\"\nother.py:3:    return 'other'\n    \"\"\"\n",
    )
    .unwrap();
    std::fs::write(
        project.join("ambiguous.py:3:other.py"),
        "def second():\n    # padding\n    return 'other'\n",
    )
    .unwrap();
    assert_eq!(
        call(
            &mut session,
            &project,
            &project,
            "ambiguous.py:3:other.py:3:    return 'other'\n"
        ),
        json!({}),
        "ambiguous numeric-colon addresses are not guessed"
    );
    std::fs::write(
        project.join("a.py"),
        "def first():\n    \"\"\"\nprefix:other.py:3:    return 'other'\n    \"\"\"\n",
    )
    .unwrap();
    std::fs::write(
        project.join("a.py:3:prefix:other.py"),
        "def second():\n    # padding\n    return 'other'\n",
    )
    .unwrap();
    assert_eq!(
        call(
            &mut session,
            &project,
            &project,
            "a.py:3:prefix:other.py:3:    return 'other'\n"
        ),
        json!({}),
        "a later colon split is ambiguous even when the first text component is not a source path"
    );

    // Colons in a filename are valid when exactly one supported address split exists.
    // An overflowing numeric field cannot count as a second plausible line.
    for path in ["one:two:chosen.py", "a.py:184467440737095516160:chosen.py"] {
        std::fs::write(project.join(path), source).unwrap();
        let result = call(
            &mut session,
            &project,
            &project,
            &format!("{path}:2:    return 'genuine'\n"),
        );
        assert!(context(&result).contains(source), "unique address: {path}");
    }
}

#[test]
fn source_hint_hook_project_routing_and_confinement() {
    let owned = tempfile::tempdir().unwrap();
    let base = owned.path().canonicalize().unwrap();
    let mut session = StdioServerSession::new(None);
    for label in ["alpha", "beta"] {
        let project = base.join(label);
        std::fs::create_dir(&project).unwrap();
        let source = format!("def {label}():\n    return 'same hint'\n");
        std::fs::write(project.join("source.py"), &source).unwrap();
        for path in [
            "source.py".to_owned(),
            project.join("source.py").to_string_lossy().into_owned(),
        ] {
            let root = call(
                &mut session,
                &project,
                &project,
                &format!("{path}:2:    return 'same hint'\n"),
            );
            let text = context(&root);
            assert!(
                text.contains(&source),
                "explicit project selects its own enclosing bytes"
            );
            assert!(!text.contains(if label == "alpha" {
                "def beta():"
            } else {
                "def alpha():"
            }));
        }
    }
    let project = base.join("alpha");
    let nested = project.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let nested_source = "def nested():\n    return 'same hint'\n";
    std::fs::write(nested.join("source.py"), nested_source).unwrap();
    let nested_result = call(
        &mut session,
        &project,
        &nested,
        "source.py:2:    return 'same hint'\n",
    );
    assert!(
        context(&nested_result).contains(nested_source),
        "relative hints resolve at their explicit base"
    );
    assert!(
        !context(&nested_result).contains("def alpha():"),
        "validating cwd without using it is insufficient"
    );
    let outside = base.join("outside.py");
    std::fs::write(&outside, "def outside():\n    return 'same hint'\n").unwrap();
    std::os::unix::fs::symlink(&outside, project.join("linked.py")).unwrap();
    for path in [
        "../outside.py".to_owned(),
        outside.to_string_lossy().into_owned(),
        "linked.py".to_owned(),
    ] {
        assert_eq!(
            call(
                &mut session,
                &project,
                &project,
                &format!("{path}:2:    return 'same hint'\n")
            ),
            json!({})
        );
    }
    assert_eq!(
        call(
            &mut session,
            &project,
            &base,
            "source.py:2:    return 'same hint'\n"
        ),
        json!({}),
        "an address base outside the explicit project is refused"
    );
    assert_eq!(
        call(
            &mut session,
            Path::new("relative-project"),
            &project,
            "source.py:2:    return 'same hint'\n"
        ),
        json!({})
    );
    // Same selected path now has different actual bytes: old text cannot authorize them.
    std::fs::write(
        project.join("source.py"),
        "def changed():\n    return 'different'\n",
    )
    .unwrap();
    assert_eq!(
        call(
            &mut session,
            &project,
            &project,
            "source.py:2:    return 'same hint'\n"
        ),
        json!({})
    );
}

#[test]
fn source_hint_hook_preserves_physical_lf_cr_unicode_and_whole_unit_bounds() {
    let owned = tempfile::tempdir().unwrap();
    let project = owned.path().canonicalize().unwrap();
    let source = "def unicode():\r\n    return '雪\u{2028}x'\t# exact\r\n";
    std::fs::write(project.join("unicode.py"), source).unwrap();
    let mut session = StdioServerSession::new(None);
    let root = call(
        &mut session,
        &project,
        &project,
        "unicode.py:2:    return '雪\u{2028}x'\t# exact\r\n",
    );
    assert!(
        context(&root).contains(source),
        "Unicode separators are not physical LF, and CR/tab bytes stay exact"
    );
    assert!(context(&root).contains(&hex_sha256(source.as_bytes())));
    let bounds = codestory_contracts::source_context::SOURCE_CONTEXT_BOUNDS;
    assert!(context(&root).len() <= bounds.display_source_bytes + bounds.display_metadata_bytes);
    let huge = format!(
        "def too_large():\n    return '{}'\n",
        "x".repeat(bounds.display_source_bytes)
    );
    std::fs::write(project.join("large.py"), &huge).unwrap();
    assert_eq!(
        call(
            &mut session,
            &project,
            &project,
            "large.py:1:def too_large():\n"
        ),
        json!({}),
        "do not clip an overbound source unit into a claimed complete unit"
    );
}
